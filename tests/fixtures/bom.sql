-- Invented, isolated test data. Never a merchant SKU/BOM or warehouse record.
INSERT INTO product_families (code) VALUES ('DEMO-FAMILY');
INSERT INTO catalogue_items VALUES
('DEMO-COVERS','bom-covers',100,'EUR','covers-only',true),
('DEMO-FILLED','bom-filled',200,'EUR','with-foam',true);
INSERT INTO catalogue_variants (sku,family_code,size_code,textile_code) VALUES
('DEMO-COVERS','DEMO-FAMILY','fixture-size','fixture-textile'),
('DEMO-FILLED','DEMO-FAMILY','fixture-size','fixture-textile');
INSERT INTO physical_components (code,kind) VALUES
('DEMO-COVER','cover'), ('DEMO-FOAM','foam');
INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',1),('DEMO-FILLED',1);
INSERT INTO catalogue_bom_lines VALUES
('DEMO-COVERS',1,'DEMO-COVER',2),
('DEMO-FILLED',1,'DEMO-COVER',2),('DEMO-FILLED',1,'DEMO-FOAM',2);
UPDATE catalogue_boms SET sealed=true;
