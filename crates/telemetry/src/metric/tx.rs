pub const TX_SUBMITTED: &str = "tx_submitted";
pub const TX_MESSAGES: &str = "tx_messages";
pub const TX_LATENCY_SUBMITTED_MS: &str = "tx_latency_submitted_ms";
pub const TX_LATENCY_CONFIRMED_MS: &str = "tx_latency_confirmed_ms";
pub const TX_ERRORS: &str = "tx_errors";
pub const TX_CONSECUTIVE_FAILURES: &str = "tx_consecutive_failures";
pub const GAS_PAID: &str = "gas_paid";
pub const TX_CHANNEL_UTILIZATION: &str = "tx_channel_utilization";

pub const TX_LATENCY_SUBMITTED_BUCKETS: [f64; 8] = [
    100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0, 30000.0,
];

pub const TX_LATENCY_CONFIRMED_BUCKETS: [f64; 7] =
    [1000.0, 2500.0, 5000.0, 10000.0, 20000.0, 30000.0, 60000.0];

pub const GAS_PAID_BUCKETS: [f64; 7] = [
    50_000.0,
    100_000.0,
    200_000.0,
    500_000.0,
    1_000_000.0,
    2_000_000.0,
    5_000_000.0,
];
