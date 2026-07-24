CREATE UNIQUE INDEX projects_by_filesystem_id
ON projects(filesystem_id)
WHERE filesystem_id IS NOT NULL;
