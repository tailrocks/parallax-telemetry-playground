CREATE TABLE IF NOT EXISTS catalog_products (
  id VARCHAR(32) PRIMARY KEY,
  sku VARCHAR(128) NOT NULL UNIQUE,
  name VARCHAR(256) NOT NULL,
  price_minor INTEGER NOT NULL
);

INSERT INTO catalog_products (id, sku, name, price_minor)
VALUES
  ('1', 'WIDGET-1', 'Widget', 1999),
  ('2', 'GADGET-1', 'Gadget', 4999)
ON CONFLICT (id) DO UPDATE
SET sku = EXCLUDED.sku,
    name = EXCLUDED.name,
    price_minor = EXCLUDED.price_minor;
