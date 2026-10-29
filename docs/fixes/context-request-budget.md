# Context request byte protection

Regression: a resumed long-running assistant session carried a historical inline
image (~1.8 MB) into later requests. Local token pressure stayed below the normal
summarization threshold while the gateway rejected the byte payload with HTTP 413.

- Add configurable compact.request_byte_budget (default 2 MiB, 0 disables the
  preflight). It measures serialized provider-neutral model/system/messages/tool
  content, not an exact promise about provider-specific wire overhead or limits.
- Project historical image blocks out of future requests; preserve the current
  turn's images, original session payloads, text and tool call/result pairings.
  Mark omitted visual content unavailable rather than inventing a description.
- Trigger summarization on byte pressure independently of tokens. Summarize older
  turns and retain the exact active turn. Text-only summary requests omit image
  bytes; existing bounded oversized-summary retries also handle HTTP 413.
- A synchronous provider HTTP 413/prompt-too-long rejection permits one request
  recovery after successful compaction. Do not replay engine.run, tools or user
  admission. Network/429 failures and stream errors do not enter this recovery.
- Repeated 413, failed/circuit-broken compaction and an oversized current payload
  fail explicitly. Disabled automatic compaction does not spend on recovery.

Compaction changes the model's working context, not the host's full chat/message
archive. Current oversized attachments are not silently discarded. A provider
may have a lower limit than configured, so recovery is bounded and the host must
surface a reduce-context action. Provider/model live validation and deployment
are separate from offline behavior tests.
