-- No backfill: absent protocol evidence never certifies a legacy Starting tree.
CREATE TABLE IF NOT EXISTS runtime_generation_custody (
    generation_uuid TEXT PRIMARY KEY REFERENCES runtime_generation(generation_uuid),
    proof_path TEXT NOT NULL,
    proof_device INTEGER NOT NULL,
    proof_inode INTEGER NOT NULL
);
