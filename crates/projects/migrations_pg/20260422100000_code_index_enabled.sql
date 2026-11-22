-- Add code_index_enabled column to projects table (PostgreSQL)
-- Defaults to true (enabled) for all existing projects.

ALTER TABLE projects ADD COLUMN code_index_enabled BOOLEAN NOT NULL DEFAULT true;
