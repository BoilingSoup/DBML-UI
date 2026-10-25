CREATE TABLE products (
  price REAL CHECK (price > 0),
  stock INTEGER NOT NULL DEFAULT 0 CHECK (stock >= 0),
  CONSTRAINT price_band CHECK (price < 1000000)
);
