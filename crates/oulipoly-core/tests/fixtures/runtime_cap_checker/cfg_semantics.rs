#[cfg(any(target_os = "macos", test))]
const MIXED_PRODUCTION_CFG: usize = 1;

#[cfg(test)]
const TEST_ONLY_CFG: usize = 2;

#[cfg(all(unix, test))]
const TEST_ONLY_CONJUNCTION: usize = 3;

#[cfg(any(test, feature = "test-support"))]
const TEST_ONLY_DISJUNCTION: usize = 4;

#[cfg(not(test))]
const EXPLICIT_PRODUCTION_CFG: usize = 5;

fn use_production_cfg() -> usize {
    MIXED_PRODUCTION_CFG + EXPLICIT_PRODUCTION_CFG
}
