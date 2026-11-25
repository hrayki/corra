use async_trait::async_trait;

use super::Provider;

pub struct Existing;

#[async_trait]
impl Provider for Existing {
    fn name(&self) -> &'static str {
        "existing"
    }

    fn creates_machines(&self) -> bool {
        false
    }
}
