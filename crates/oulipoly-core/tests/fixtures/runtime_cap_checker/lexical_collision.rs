fn first_wait() {
    const RETRY_INTERVAL: u64 = 10;
    consume(RETRY_INTERVAL);
}

fn second_wait() {
    const RETRY_INTERVAL: u64 = 20;
    consume(RETRY_INTERVAL);
}

fn nested_wait(use_inner: bool) {
    const RETRY_INTERVAL: u64 = 30;
    consume(RETRY_INTERVAL);
    if use_inner {
        const RETRY_INTERVAL: u64 = 40;
        consume(RETRY_INTERVAL);
    }
    consume(RETRY_INTERVAL);
}
