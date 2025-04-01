CREATE DATABASE lor_e;

\c lor_e;

CREATE EXTENSION vector;

CREATE TABLE IF NOT EXISTS issues (
  id SERIAL PRIMARY KEY,
  source_id VARCHAR NOT NULL UNIQUE,
  source VARCHAR NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  is_pull_request BOOLEAN NOT NULL,
  number INT NOT NULL,
  html_url VARCHAR NOT NULL,
  url VARCHAR NOT NULL,
  embedding vector(1024) NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE TABLE IF NOT EXISTS comments (
  id SERIAL PRIMARY KEY,
  source_id VARCHAR NOT NULL UNIQUE,
  issue_id INT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  body TEXT NOT NULL,
  url VARCHAR NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE INDEX IF NOT EXISTS issues_source_id_idx ON issues (source_id);
CREATE INDEX IF NOT EXISTS comments_source_id_idx ON comments (source_id);
CREATE INDEX IF NOT EXISTS issues_embedding_hnsw_idx ON issues USING hnsw (embedding vector_cosine_ops);

CREATE TABLE IF NOT EXISTS jobs (
  id SERIAL PRIMARY KEY,
  repository_id VARCHAR NOT NULL UNIQUE,
  page INT NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE INDEX IF NOT EXISTS jobs_repository_id_idx ON jobs (repository_id);
