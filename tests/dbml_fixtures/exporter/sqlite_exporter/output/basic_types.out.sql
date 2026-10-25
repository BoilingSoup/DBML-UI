CREATE TABLE "users" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "email" varchar(255) UNIQUE NOT NULL,
  "display_name" varchar(120),
  "age" int DEFAULT 18,
  "rating" double DEFAULT 0.0,
  "settings" json DEFAULT '{}',
  "created_at" timestamp NOT NULL DEFAULT (now()),
  "active" boolean NOT NULL DEFAULT true
);

