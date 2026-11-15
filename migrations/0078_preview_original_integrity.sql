-- One fixed-recipe integrity assertion; no original bytes or rights authority.
-- Provisioning a non-login owner and observer EXECUTE remains an operator task.
CREATE FUNCTION public.preview_original_intact(wanted text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
 IF wanted IS NULL OR octet_length(wanted) NOT BETWEEN 1 AND 128
  OR wanted !~ '^[a-zA-Z0-9-]{1,128}$' THEN
  RAISE EXCEPTION 'Invalid preview asset' USING ERRCODE='22023'; END IF;
 RETURN COALESCE((SELECT CASE WHEN octet_length(o.bytes) BETWEEN 1 AND 20971520
  AND octet_length(o.sha256)=32 THEN o.sha256=pg_catalog.sha256(o.bytes) ELSE false END
  FROM public.catalogue_image_originals o WHERE o.asset_id=wanted AND o.recipe='image-v1'),false);
END $$;
REVOKE ALL ON FUNCTION public.preview_original_intact(text) FROM PUBLIC;
-- Remove named default grantees on this new helper without altering defaults.
DO $$
DECLARE entry record;
BEGIN
 FOR entry IN
  SELECT r.rolname FROM pg_catalog.pg_proc p
  CROSS JOIN LATERAL pg_catalog.aclexplode(p.proacl) a
  JOIN pg_catalog.pg_roles r ON r.oid=a.grantee
  WHERE p.oid='public.preview_original_intact(text)'::pg_catalog.regprocedure
   AND a.grantee<>p.proowner
 LOOP
  EXECUTE pg_catalog.format('REVOKE ALL ON FUNCTION public.preview_original_intact(text) FROM %I', entry.rolname);
 END LOOP;
END $$;
