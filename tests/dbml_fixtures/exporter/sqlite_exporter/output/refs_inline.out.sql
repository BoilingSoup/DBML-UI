CREATE TABLE "users" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "country_id" int NOT NULL
);

CREATE TABLE "countries" (
  "id" integer PRIMARY KEY,
  "editor_id" int
);

CREATE TABLE "posts" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "author_id" integer NOT NULL,
  "country_id" int,
  "editor_id" int,
  FOREIGN KEY ("author_id") REFERENCES "users" ("id"),
  FOREIGN KEY ("country_id") REFERENCES "countries" ("id"),
  FOREIGN KEY ("country_id", "editor_id") REFERENCES "countries" ("id", "editor_id") ON DELETE CASCADE ON UPDATE SET NULL
);

