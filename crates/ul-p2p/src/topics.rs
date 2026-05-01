pub const PROTOCOL_PREFIX: &str = "tally-free";
pub const PROTOCOL_VERSION: &str = "v1";

pub const TOPIC_TX: &str = "tally-free/tx/v1";
pub const TOPIC_PROPOSAL: &str = "tally-free/proposal/v1";
pub const TOPIC_VOTE: &str = "tally-free/vote/v1";
pub const TOPIC_COMMIT: &str = "tally-free/commit/v1";
pub const TOPIC_PEER_CAPACITY: &str = "tally-free/peer-capacity/v1";
pub const TOPIC_SYNC: &str = "tally-free/sync/v1";

pub fn all_topics() -> [&'static str; 6] {
    [TOPIC_TX, TOPIC_PROPOSAL, TOPIC_VOTE, TOPIC_COMMIT, TOPIC_PEER_CAPACITY, TOPIC_SYNC]
}
