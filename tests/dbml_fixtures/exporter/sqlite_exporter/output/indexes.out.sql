CREATE TABLE "events" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "user_id" int,
  "occurred_at" timestamp,
  "kind" varchar(40)
);

CREATE INDEX "events_user_time" ON "events" ("user_id", "occurred_at");

CREATE UNIQUE INDEX "events_kind_unique" ON "events" ("kind");

