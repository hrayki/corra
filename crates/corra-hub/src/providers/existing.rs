use async_trait::async_trait;

use super::{Description, Provider};

pub struct Existing;

#[async_trait]
impl Provider for Existing {
    fn name(&self) -> &'static str {
        "existing"
    }

    fn describe(&self) -> Description {
        Description {
            label: "Paired machine",
            about: "Any Linux computer running corra's runner. Corra never starts or stops it.",
            credentials: &[],
            kinds: vec![],
            listed: true,
        }
    }

    fn creates_machines(&self) -> bool {
        false
    }
}
