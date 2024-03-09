use std::time::Instant;

use crate::sample::Sample;

#[derive(Debug)]
pub struct Profile {
    pub start_timestamp: Instant,
    pub samples: Vec<Sample>,
}
