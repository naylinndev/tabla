-- Test fixture for the Postgres integration tests (run against a throwaway database).
CREATE TYPE mood AS ENUM ('happy', 'sad');
CREATE TABLE users (
  id serial PRIMARY KEY, email text NOT NULL UNIQUE, name varchar(80), age int, balance numeric(12,2),
  active boolean DEFAULT true, profile jsonb, tags text[], feeling mood, created_at timestamptz DEFAULT now(), avatar bytea
);
INSERT INTO users (email, name, age, balance, profile, tags, feeling, avatar)
SELECT 'user' || g || '@example.com', 'User ' || g, 18 + g % 50, g * 10.5,
       jsonb_build_object('plan', CASE WHEN g % 3 = 0 THEN 'pro' ELSE 'free' END, 'score', g),
       ARRAY['a', 'b'], CASE WHEN g % 2 = 0 THEN 'happy'::mood ELSE 'sad' END, '\xdeadbeef'
FROM generate_series(1, 1200) g;
CREATE TABLE empty_t (id int PRIMARY KEY, note text);
CREATE VIEW active_users AS SELECT id, email FROM users WHERE active;
