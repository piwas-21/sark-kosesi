-- Read historical custody without requiring a live or unconsumed challenge.
SELECT s.verifier IS NOT NULL, o.verifier IS NOT NULL, left(o.outcome,20),
 CASE WHEN o.start_transaction_id=s.transaction_id AND o.transaction_id<>s.transaction_id
  AND o.observed_at>=s.started_at AND o.observed_at<=9007199254740991 THEN
  CASE
   WHEN o.evidence_version=0 AND o.ciphertext_sha256 IS NULL THEN 0::smallint
   WHEN o.evidence_version=1 AND EXISTS(
    SELECT 1 FROM email_verification_delivery_envelopes e
    WHERE e.verifier=s.verifier AND e.key_id=s.key_id AND e.nonce_counter=s.nonce_counter
     AND CASE WHEN e.envelope_version=1 AND octet_length(e.ciphertext)=48
      THEN sha256(e.ciphertext)=o.ciphertext_sha256 ELSE false END
   ) THEN 1::smallint
  END
 END
FROM (SELECT $1::bytea AS verifier) requested
LEFT JOIN email_verification_delivery_starts s ON s.verifier=requested.verifier
LEFT JOIN email_verification_delivery_outcomes o ON o.verifier=requested.verifier
