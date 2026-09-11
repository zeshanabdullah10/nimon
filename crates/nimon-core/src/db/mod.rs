//! Database layer for NIMon

pub mod action_repo;
pub mod alert_repo;
pub mod device_repo;
pub mod edge_repo;
pub mod prediction_repo;
pub mod schema;

pub use action_repo::{ActionRecord, ActionRepository};
pub use alert_repo::{AlertRecord, AlertRepository};
pub use prediction_repo::{PredictionRecord, PredictionRepository};

pub use schema::init_database;

#[cfg(test)]
pub use schema::create_test_db;
