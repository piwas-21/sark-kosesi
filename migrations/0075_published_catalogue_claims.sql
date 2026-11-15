-- Narrow catalogue read claims. The migration owner retains ordinary execution;
-- provisioning a separate non-login owner and runtime EXECUTE is an operations task.
CREATE FUNCTION public.claim_published_variants(wanted text[]) RETURNS SETOF text
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
 IF wanted IS NULL OR cardinality(wanted)>25 OR
    (cardinality(wanted)>0 AND array_ndims(wanted)<>1) OR
    EXISTS (SELECT 1 FROM unnest(wanted) AS value WHERE value IS NULL OR octet_length(value) NOT BETWEEN 1 AND 65) THEN
  RAISE EXCEPTION 'Invalid published variant claim' USING ERRCODE='22023';
 END IF;
 RETURN QUERY SELECT v.sku FROM public.catalogue_variants v
  WHERE v.sku=ANY(wanted) ORDER BY v.sku FOR SHARE;
END $$;

CREATE FUNCTION public.claim_published_assets(wanted text[]) RETURNS SETOF text
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
BEGIN
 IF wanted IS NULL OR cardinality(wanted)>25 OR
    (cardinality(wanted)>0 AND array_ndims(wanted)<>1) OR
    EXISTS (SELECT 1 FROM unnest(wanted) AS value WHERE value IS NULL OR octet_length(value) NOT BETWEEN 1 AND 128) THEN
  RAISE EXCEPTION 'Invalid published asset claim' USING ERRCODE='22023';
 END IF;
 RETURN QUERY SELECT s.asset_id FROM public.catalogue_media_serialization s
  WHERE s.asset_id=ANY(wanted) ORDER BY s.asset_id FOR SHARE;
END $$;
REVOKE ALL ON FUNCTION public.claim_published_variants(text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.claim_published_assets(text[]) FROM PUBLIC;
-- Default ACLs may also grant named roles. These functions are new, so remove
-- every inherited grantee except the owner without changing global defaults.
DO $$
DECLARE entry record;
BEGIN
 FOR entry IN
  SELECT p.proname, r.rolname
  FROM pg_catalog.pg_proc p
  JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
  CROSS JOIN LATERAL pg_catalog.aclexplode(p.proacl) a
  JOIN pg_catalog.pg_roles r ON r.oid=a.grantee
  WHERE n.nspname='public' AND p.oid IN
   ('public.claim_published_variants(text[])'::pg_catalog.regprocedure,
    'public.claim_published_assets(text[])'::pg_catalog.regprocedure)
   AND a.grantee<>p.proowner
 LOOP
  EXECUTE pg_catalog.format('REVOKE ALL ON FUNCTION public.%I(text[]) FROM %I', entry.proname, entry.rolname);
 END LOOP;
END $$;
