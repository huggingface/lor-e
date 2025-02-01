CREATE TABLE IF NOT EXISTS issues (
  id SERIAL PRIMARY KEY,
  github_id BIGINT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  issue_type VARCHAR NOT NULL CHECK (issue_type IN ('issue', 'pull_request')),
  url VARCHAR NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE TABLE IF NOT EXISTS issue_comments (
  id SERIAL PRIMARY KEY,
  github_id BIGINT NOT NULL,
  issue_id INT NOT NULL REFERENCES issues(id),
  body TEXT NOT NULL,
  url VARCHAR NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE TABLE IF NOT EXISTS pull_request_reviews (
  id SERIAL PRIMARY KEY,
  github_id BIGINT NOT NULL,
  issue_id INT NOT NULL REFERENCES issues(id),
  body TEXT NOT NULL,
  url VARCHAR NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE TABLE IF NOT EXISTS pull_request_review_comments (
  id SERIAL PRIMARY KEY,
  github_id BIGINT NOT NULL,
  issue_id INT NOT NULL REFERENCES issues(id),
  body TEXT NOT NULL,
  url VARCHAR NOT NULL,
  created_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC'),
  updated_at timestamp with time zone NOT NULL DEFAULT (current_timestamp AT TIME ZONE 'UTC')
);

CREATE INDEX IF NOT EXISTS issues_github_id_idx ON issues (github_id);
CREATE INDEX IF NOT EXISTS issue_comments_github_id_idx ON issue_comments (github_id);
CREATE INDEX IF NOT EXISTS pull_request_reviews_github_id_idx ON pull_request_reviews (github_id);
CREATE INDEX IF NOT EXISTS pull_request_review_comments_github_id_idx ON pull_request_review_comments (github_id);
