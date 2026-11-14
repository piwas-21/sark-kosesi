-- A bounded live scan over review stops; no grants, state changes or effects.
CREATE INDEX reversal_item_review_inbox_cursor
 ON reversal_item_jobs (updated_at, id COLLATE "C") WHERE state = 'review';
