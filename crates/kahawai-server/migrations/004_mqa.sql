-- MQA detection. MQA-encoded FLAC files carry MQAENCODER / ORIGINALSAMPLERATE
-- tags; the scanner records them so clients can label the track and, for a
-- DAC that decodes MQA, play it bit-perfect.
--
-- mqa_checked = 0 marks rows cataloged before this column existed: the next
-- scan reads just their tags (no re-hash) and sets it. Only FLAC can carry
-- these tags in the way we detect, so every other format is checked already.

ALTER TABLE tracks ADD COLUMN mqa INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tracks ADD COLUMN original_sample_rate INTEGER;
ALTER TABLE tracks ADD COLUMN mqa_checked INTEGER NOT NULL DEFAULT 0;
UPDATE tracks SET mqa_checked = 1 WHERE format != 'flac';
