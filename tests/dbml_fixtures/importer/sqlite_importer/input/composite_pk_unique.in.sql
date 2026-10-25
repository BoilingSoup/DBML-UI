CREATE TABLE subscriptions (
  user_id INTEGER NOT NULL,
  plan TEXT NOT NULL,
  started_at TEXT,
  PRIMARY KEY (user_id, plan),
  UNIQUE (started_at, user_id)
) WITHOUT ROWID;
