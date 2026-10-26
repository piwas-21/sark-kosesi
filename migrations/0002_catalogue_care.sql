-- Expand only: old readers remain compatible; existing translations remain intact.
ALTER TABLE catalogue_translations ADD COLUMN care text NOT NULL DEFAULT '';
ALTER TABLE catalogue_translations ADD CONSTRAINT bounded_care CHECK (length(care) <= 2000);
