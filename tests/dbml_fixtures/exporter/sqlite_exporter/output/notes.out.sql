-- note: append-only audit trail
CREATE TABLE "audit_log" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  -- note: who performed the action
  "actor" varchar(120),
  "payload" json
);

