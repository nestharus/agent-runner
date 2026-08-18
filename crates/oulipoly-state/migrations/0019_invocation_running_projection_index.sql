-- Bound running-first invocation projection without sorting all terminal history.
-- ## Declared roles
-- `mapper`

-- The registered migration hook applies the index when the incoming
-- invocations table already has the schema-4 projection columns. The exact
-- supported pre-UUID shape is rebuilt by the sanctioned schema repair after
-- ordered migrations, which applies the same index from the current schema SQL.
SELECT 1;
