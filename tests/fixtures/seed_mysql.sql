-- Test fixture for the MySQL integration tests (run against a throwaway database).
CREATE TABLE products (
  id INT AUTO_INCREMENT PRIMARY KEY, sku VARCHAR(32) UNIQUE, title VARCHAR(200) DEFAULT 'untitled',
  price DECIMAL(10,2), stock INT, meta JSON, img BLOB, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO products (sku, title, price, stock, meta, img) VALUES
  ('A1', 'Keyboard', 49.90, 10, '{"color":"black"}', X'89504E47'),
  ('B2', 'Mouse', 19.00, NULL, NULL, NULL),
  ('C3', 'Monitor ''27"', 299.00, 3, '[1,2]', NULL);
