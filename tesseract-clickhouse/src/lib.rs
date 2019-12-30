use clickhouse_rs::Pool;
use clickhouse_rs::types::{Options, Simple, Complex, Block};
use failure::{Error, format_err};
use futures::{future, Future, Stream};
use log::*;
use std::time::{Duration, Instant};
use tesseract_core::{Backend, DataFrame, QueryIr};

mod df;
mod sql;

use self::df::{block_to_df};
use self::sql::clickhouse_sql;

// Ping timeout in millis
const PING_TIMEOUT: u64 = 100_000;

#[derive(Clone)]
pub struct Clickhouse {
    pool: Pool,
}

impl Clickhouse {
    pub fn from_url(url: &str) -> Result<Self, Error> {
        let options = format!("tcp://{}", url).parse::<Options>()?;

        let options = options
            // Ping timeout is necessary, because under heavy load (100 requests
            // simultaneously, each one taking 5s ordinarily) the client will timeout.
            .ping_timeout(Duration::from_millis(PING_TIMEOUT));

        let pool = Pool::new(options);

        Ok(Clickhouse {
            pool,
        })
    }
}

impl Backend for Clickhouse {
    fn exec_sql(&self, sql: String) -> Box<dyn Future<Item=DataFrame, Error=Error>> {
        let time_start = Instant::now();

        let fut = self.pool
            .get_handle()
            .and_then(move |c| c.query(&sql[..]).fetch_all())
            .from_err()
            .and_then(move |(_, block): (_, Block<Complex>)| {
                let timing = time_start.elapsed();
                info!("Time for sql execution: {}.{:03}", timing.as_secs(), timing.subsec_millis());
                //debug!("Block: {:?}", block);

                Ok(block_to_df(block)?)
            });

        Box::new(fut)
    }

    fn exec_sql_stream(&self, sql: String) -> Box<dyn Stream<Item=Result<DataFrame, Error>, Error=Error>> {
        let fut_stream = self.pool
            .get_handle()
            .and_then(move |c| {
                future::ok(
                    c.query(&sql[..])
                        .stream_blocks()
                        .map(move |block: Block<Simple>| {
                            block_to_df(block)
                        })
                )
            })
            .flatten_stream()
            .map_err(|err| format_err!("{}", err));

        Box::new(fut_stream)
    }

    // https://users.rust-lang.org/t/solved-is-it-possible-to-clone-a-boxed-trait-object/1714/4
    fn box_clone(&self) -> Box<dyn Backend + Send + Sync> {
        Box::new((*self).clone())
    }

    fn generate_sql(&self, query_ir: QueryIr) -> String {
        clickhouse_sql(
            &query_ir
        )
    }

    fn check_user(&self) -> Box<dyn Future<Item=(), Error= Error>> {
        let index_sql = "select Settings.Values as values, Settings.Names as names from system.processes";
        let f = self.pool
            .get_handle()
            .and_then(move |c| c.query(index_sql).fetch_all())
            .and_then(move |(_, block): (_, Block<Complex>)| {
                let values: Vec<&str> = block.get(0, "values")?;
                let names: Vec<&str> = block.get(0, "names")?;
                if names.contains(&"readonly") && names.contains(&"allow_ddl") {
                    let rd_index = names.iter().position(|&r| r=="readonly").unwrap();
                    let ad_index = names.iter().position(|&r| r=="allow_ddl").unwrap();
                    // let rd_value = values.get(rd_index).parse::<u32>().unwrap();
                    // let ad_value = values.get(ad_index).parse::<u32>().unwrap();
                    let rd_value = values.get(rd_index).unwrap();
                    let ad_value = values.get(ad_index).unwrap();
                    if rd_value != &"1" && ad_value != &"0" {
                        warn!("Warning: Database connection has write access. Users may be able to modify data.");
                    }
                } else {
                    warn!("Warning: Database connection has write access. Users may be able to modify data.");
                }
                Ok(())
            })
            .map_err(|err| format_err!("{}", err));
        Box::new(f)
    }
}
