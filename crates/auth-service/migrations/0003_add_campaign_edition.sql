-- Which edition's rules a campaign plays by ('2014' or '2024'). New
-- campaigns always name one; existing ones get 2024.
ALTER TABLE campaigns ADD COLUMN edition TEXT NOT NULL DEFAULT '2024';
