use common::channels::Response;
use common::shard_id::ShardId;

pub enum MultiKeyResult {
    Success(i64),
    PartialFailure {
        succeeded: i64,
        failed_shards: Vec<ShardId>,
        errors: Vec<String>,
    },
    TotalFailure(String),
}

impl MultiKeyResult {
    pub fn to_response(self) -> Response {
        match self {
            MultiKeyResult::Success(n) => Response::Integer(n),
            MultiKeyResult::PartialFailure { succeeded, failed_shards, errors } => {
                let msg = format!(
                    "ERR partial failure: {} succeeded, {} failed on shards {:?}: {}",
                    succeeded,
                    failed_shards.len(),
                    failed_shards,
                    errors.join(", ")
                );
                Response::Error(msg)
            }
            MultiKeyResult::TotalFailure(msg) => Response::Error(msg),
        }
    }
}