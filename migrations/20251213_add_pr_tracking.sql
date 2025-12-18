-- Add PR tracking columns to updates table
ALTER TABLE updates ADD COLUMN pr_url TEXT;
ALTER TABLE updates ADD COLUMN pr_number INTEGER;
