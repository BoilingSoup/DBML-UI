CREATE TABLE "students" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT
);

CREATE TABLE "courses" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT
);

CREATE TABLE "students_courses" (
  "students_id" integer,
  "courses_id" integer,
  PRIMARY KEY ("students_id", "courses_id"),
  FOREIGN KEY ("students_id") REFERENCES "students" ("id"),
  FOREIGN KEY ("courses_id") REFERENCES "courses" ("id")
);

