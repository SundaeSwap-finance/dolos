-- A LeiosDb layout no node writes, the ebs table of node release rc-2026w45 without its other tables.
CREATE TABLE IF NOT EXISTS ebs (
  ebSlot INTEGER NOT NULL,
  ebHashBytes BLOB NOT NULL,
  ebBytesSize INTEGER NOT NULL,
  PRIMARY KEY (ebSlot, ebHashBytes)
);
