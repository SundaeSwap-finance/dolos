-- The schemas sql_schema_imm and sql_schema_vol of ouroboros-consensus 8a2a760c, which are the same.
CREATE TABLE IF NOT EXISTS ebs (
  ebSlot INTEGER NOT NULL,
  ebHashBytes BLOB NOT NULL,
  txWritten BLOB NOT NULL,
  txBounds BLOB NOT NULL,
  body BLOB NOT NULL,
  txs BLOB NOT NULL,
  PRIMARY KEY (ebSlot, ebHashBytes)
);
