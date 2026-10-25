CREATE TABLE users (
  id INTEGER PRIMARY KEY AUTOINCREMENT
);

CREATE TABLE countries (
  id integer PRIMARY KEY
);

CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  author_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE ON UPDATE RESTRICT,
  country_id int,
  editor_id int,
  FOREIGN KEY (country_id) REFERENCES countries (id),
  FOREIGN KEY (country_id, editor_id) REFERENCES countries (id, editor_id) DEFERRABLE INITIALLY DEFERRED
);
