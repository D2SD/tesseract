use actix_web::{
    AsyncResponder,
    FutureResponse,
    HttpRequest,
    HttpResponse,
    Path,
};
use failure::{Error, format_err};
use futures::future::{self, Future};
use lazy_static::lazy_static;
use log::*;
use serde_derive::{Serialize, Deserialize};
use serde_qs as qs;
use std::convert::{TryFrom, TryInto};
use tesseract_core::{Schema, Cube};
use tesseract_core::format::{format_records, FormatType};
use tesseract_core::Query as TsQuery;
use tesseract_core::names::{LevelName, Measure as MeasureName};

use crate::app::AppState;
use crate::handlers::aggregate::AggregateQueryOpt;
use crate::handlers::logic_layer::aggregate::finish_aggregation;


/// Handles default aggregation when a format is not specified.
/// Default format is CSV.
pub fn cube_detection_aggregation_default_handler(
    (req, cube): (HttpRequest<AppState>, Path<()>)
) -> FutureResponse<HttpResponse>
{
    do_cube_detection_aggregation(req, "csv".to_owned())
}

/// Handles aggregation when a format is specified.
pub fn cube_detection_aggregation_handler(
    (req, cube_format): (HttpRequest<AppState>, Path<(String)>)
) -> FutureResponse<HttpResponse>
{
    do_cube_detection_aggregation(req, cube_format.to_owned())
}

/// Performs first step of data aggregation, including cube detection.
pub fn do_cube_detection_aggregation(
    req: HttpRequest<AppState>,
    format: String,
) -> FutureResponse<HttpResponse>
{
    let format = format.parse::<FormatType>();
    let format = match format {
        Ok(f) => f,
        Err(err) => {
            return Box::new(
                future::result(
                    Ok(HttpResponse::NotFound().json(err.to_string()))
                )
            );
        },
    };

    info!("format: {:?}", format);

    let query = req.query_string();
    lazy_static!{
        static ref QS_NON_STRICT: qs::Config = qs::Config::new(5, false);
    }
    let agg_query_res = QS_NON_STRICT.deserialize_str::<AggregateQueryOpt>(&query);
    let agg_query = match agg_query_res {
        Ok(q) => q,
        Err(err) => {
            return Box::new(
                future::result(
                    Ok(HttpResponse::NotFound().json(err.to_string()))
                )
            );
        },
    };

    // Detect cube based on the query parameters
    let cube = detect_cube(
        req.state().schema.read().unwrap().clone(),
        agg_query.clone()
    );
    let cube = match cube {
        Ok(cube) => cube,
        Err(err) => {
            return Box::new(
                future::result(
                    Ok(HttpResponse::NotFound().json(err.to_string()))
                )
            );
        }
    };
    info!("cube: {:?}", cube);

    finish_aggregation(req, agg_query, cube, format)
}

/// Detects which cube to use based on the drilldowns, cuts and measures provided.
/// In case the arguments are present in more than one cube, the first cube to match all
/// requirements is returned.
pub fn detect_cube(schema: Schema, agg_query: AggregateQueryOpt) -> Result<String, Error> {
    let drilldowns = match agg_query.drilldowns {
        Some(drilldowns) => {
            let mut d: Vec<LevelName> = vec![];
            for drilldown in drilldowns {
                let e: Vec<&str> = drilldown.split(".").collect();
                let ln = match LevelName::from_vec(e) {
                    Ok(ln) => ln,
                    Err(_) => break,
                };
                d.push(ln);
            }
            d
        },
        None => vec![],
    };

    let cuts = match agg_query.cuts {
        Some(cuts) => {
            let mut c: Vec<LevelName> = vec![];
            for cut in cuts {
                let e: Vec<&str> = cut.split(".").collect();
                // TODO: Fix
                let ln = match LevelName::from_vec(
                    e[..e.len()-1].to_vec()
                ) {
                    Ok(ln) => ln,
                    Err(_) => break,
                };
                c.push(ln);
            }
            c
        },
        None => vec![],
    };

    let measures = match agg_query.measures {
        Some(measures) => {
            let mut m: Vec<MeasureName> = vec![];
            for measure in measures {
                m.push(MeasureName::new(measure));
            }
            m
        },
        None => vec![],
    };

    // TODO: Avoid clone here?
    for cube in schema.cubes {
        let level_names = cube.get_all_level_names();
        let measure_names = cube.get_all_measure_names();

        // If this is true, we already know this is not the right cube, so need
        // to continue to next iteration of the loop
        let mut exit = false;

        for drilldown in &drilldowns {
            if !level_names.contains(drilldown) {
                exit = true;
                break;
            }
        }

        if exit {
            continue;
        }

        for cut in &cuts {
            if !level_names.contains(cut) {
                exit = true;
                break;
            }
        }

        if exit {
            continue;
        }

        for measure in &measures {
            if !measure_names.contains(measure) {
                break;
            }
        }

        return Ok(String::from(cube.name));
    }

    Err(format_err!("No cubes found with the requested drilldowns/cuts/measures."))
}
