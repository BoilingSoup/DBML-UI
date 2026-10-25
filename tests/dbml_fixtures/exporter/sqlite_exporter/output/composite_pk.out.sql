CREATE TABLE "subscriptions" (
  "user_id" int NOT NULL,
  "plan" varchar(40) NOT NULL,
  "started_at" timestamp DEFAULT (now()),
  PRIMARY KEY ("user_id", "plan")
);

