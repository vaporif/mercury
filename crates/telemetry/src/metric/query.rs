// TODO: wire into RpcGuard once query_type labels are added
#[allow(dead_code)]
pub const QUERIES: &str = "queries";
#[allow(dead_code)]
pub const QUERY_LATENCY_MS: &str = "query_latency_ms";

#[allow(dead_code)]
pub const QUERY_LATENCY_BUCKETS: [f64; 15] = [
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10_000.0, 25_000.0,
    50_000.0, 100_000.0,
];
