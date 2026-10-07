-- Remove the unsolicited catalog feeds. Preserve user intent and any source
-- evidence still needed by explicitly followed or independently opened titles.
CREATE TEMP TABLE removed_catalog_media AS
SELECT m.media_id FROM media m
WHERE NOT EXISTS (SELECT 1 FROM follows f WHERE f.media_id = m.media_id)
  AND EXISTS (
    SELECT 1 FROM source_media sm JOIN source_observations so USING (source_media_id)
    JOIN source_fetches sf USING (fetch_id)
    WHERE sm.media_id = m.media_id AND sf.request_fingerprint LIKE 'discovery:%'
  )
  AND NOT EXISTS (
    SELECT 1 FROM source_media sm JOIN source_observations so USING (source_media_id)
    JOIN source_fetches sf USING (fetch_id)
    WHERE sm.media_id = m.media_id AND sf.request_fingerprint NOT LIKE 'discovery:%'
  );

DROP TABLE discovery_feeds;
DROP TABLE discovery_members;
DROP TABLE discovery_snapshots;
DELETE FROM operations WHERE json_extract(target_json, '$.kind') = 'discovery';

DELETE FROM notification_jobs WHERE release_event_id IN (
    SELECT release_event_id FROM release_events WHERE media_id IN removed_catalog_media
);
DELETE FROM release_events WHERE media_id IN removed_catalog_media;
DELETE FROM source_refresh_state WHERE source_media_id IN (
    SELECT source_media_id FROM source_media WHERE media_id IN removed_catalog_media
);
UPDATE source_media SET current_observation_id = NULL WHERE media_id IN removed_catalog_media;
DELETE FROM source_observations WHERE source_media_id IN (
    SELECT source_media_id FROM source_media WHERE media_id IN removed_catalog_media
);
DELETE FROM source_media WHERE media_id IN removed_catalog_media;
DELETE FROM media WHERE media_id IN removed_catalog_media;
DROP TABLE removed_catalog_media;

-- Remove unreferenced catalog observations even when the title was followed.
DELETE FROM source_observations
WHERE fetch_id IN (SELECT fetch_id FROM source_fetches WHERE request_fingerprint LIKE 'discovery:%')
  AND observation_id NOT IN (SELECT current_observation_id FROM source_media WHERE current_observation_id IS NOT NULL)
  AND observation_id NOT IN (SELECT last_observation_id FROM release_events);
DELETE FROM source_fetches
WHERE request_fingerprint LIKE 'discovery:%'
  AND NOT EXISTS (SELECT 1 FROM source_observations so WHERE so.fetch_id = source_fetches.fetch_id);
