use serde::{Deserialize, Serialize};

pub const TRACE_COUNT: u64 = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub sequence: u64,
    pub event_id: [u8; 16],
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent_span_id: [u8; 8],
    pub process_root_id: [u8; 8],
    pub timestamp_micros: u64,
    pub producer: u32,
    pub family: u8,
    pub payload: Vec<u8>,
}

impl Event {
    pub fn encoded(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

pub fn fixture_events(records: usize, producers: usize, payload_bytes: usize) -> Vec<Event> {
    (0..records)
        .map(|sequence| {
            let sequence = sequence as u64;
            let producer = (sequence % producers as u64) as u32;
            let trace = sequence % TRACE_COUNT;
            Event {
                sequence,
                event_id: deterministic_id(0x4556_454e_5400_0001, sequence),
                trace_id: deterministic_id(0x5452_4143_4500_0001, trace),
                span_id: sequence.to_be_bytes(),
                parent_span_id: sequence.saturating_sub(1).to_be_bytes(),
                process_root_id: (u64::from(producer) + 1).to_be_bytes(),
                timestamp_micros: 1_800_000_000_000_000 + sequence * 1_000,
                producer,
                family: (sequence % 4) as u8,
                payload: deterministic_payload(sequence, payload_bytes),
            }
        })
        .collect()
}

pub fn target_trace() -> [u8; 16] {
    deterministic_id(0x5452_4143_4500_0001, 17)
}

pub fn time_bounds(records: usize) -> (u64, u64) {
    let start_sequence = records as u64 * 45 / 100;
    let end_sequence = records as u64 * 55 / 100;
    (
        1_800_000_000_000_000 + start_sequence * 1_000,
        1_800_000_000_000_000 + end_sequence * 1_000,
    )
}

fn deterministic_id(namespace: u64, value: u64) -> [u8; 16] {
    let mut id = [0_u8; 16];
    id[..8].copy_from_slice(&namespace.to_be_bytes());
    id[8..].copy_from_slice(&value.to_be_bytes());
    id
}

fn deterministic_payload(sequence: u64, bytes: usize) -> Vec<u8> {
    let mut state = sequence ^ 0x9e37_79b9_7f4a_7c15;
    let mut payload = Vec::with_capacity(bytes);
    for _ in 0..bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        payload.push((state & 0xff) as u8);
    }
    payload
}
