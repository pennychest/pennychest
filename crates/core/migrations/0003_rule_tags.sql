-- A rule can also require a tag, such as who paid, for descriptions that mean different
-- things for different people.

ALTER TABLE rules ADD COLUMN tag TEXT;
