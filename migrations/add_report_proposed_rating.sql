-- Rating proposals: a "wrong rating" report can carry the rating the reporter suggests (sfw/nsfw/nsfl).
ALTER TABLE reports ADD COLUMN IF NOT EXISTS proposed_rating text;
