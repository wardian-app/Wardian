import { timingSafeEqual, createHash } from 'node:crypto';

export const MAX_FRAME_BYTES = 65536;
export const CUSTOM_TYPE = 'wardian.peer-message.v1';
const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
export function exact(value, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)
      || Object.keys(value).sort().join(',') !== [...keys].sort().join(',')) throw new Error('invalid_shape');
}
export function sameSecret(a, b) {
  return typeof a === 'string' && typeof b === 'string'
    && Buffer.byteLength(a) === Buffer.byteLength(b)
    && timingSafeEqual(Buffer.from(a), Buffer.from(b));
}

/** Child-local configuration; no endpoint discovery or global configuration. */
export function parseConfig(raw) {
  if (typeof raw !== 'string' || raw.length > 8192) throw new Error('invalid_config');
  const c = JSON.parse(raw);
  exact(c, ['version', 'host', 'port', 'token', 'target_id', 'generation', 'session_id', 'session_file']);
  if (c.version !== 1 || c.host !== '127.0.0.1' || !Number.isInteger(c.port) || c.port < 1 || c.port > 65535
      || !/^[a-f0-9]{64}$/.test(c.token) || !id(c.target_id) || !id(c.session_id)
      || !Number.isSafeInteger(c.generation) || c.generation < 1
      || typeof c.session_file !== 'string' || !c.session_file || c.session_file.length > 4096
      || /[\x00-\x1f]/.test(c.session_file)) throw new Error('invalid_config');
  return Object.freeze(c);
}

/** Four-byte unsigned big-endian byte length followed by strict UTF-8 JSON. */
export function encodeFrame(value) {
  const body = Buffer.from(JSON.stringify(value));
  if (!body.length || body.length > MAX_FRAME_BYTES) throw new Error('frame_size');
  const prefix = Buffer.alloc(4);
  prefix.writeUInt32BE(body.length);
  return Buffer.concat([prefix, body]);
}

/** Incremental decoder allocates at most one bounded frame, even for coalesced input. */
export class FrameDecoder {
  constructor() { this.prefix = Buffer.alloc(4); this.offset = 0; this.body = null; this.failed = false; }
  feed(chunk, emit) {
    if (this.failed) throw new Error('decoder_closed');
    try {
      for (let cursor = 0; cursor < chunk.length;) {
        const dest = this.body ?? this.prefix;
        const count = Math.min(dest.length - this.offset, chunk.length - cursor);
        chunk.copy(dest, this.offset, cursor, cursor + count);
        cursor += count;
        this.offset += count;
        if (this.offset !== dest.length) continue;
        this.offset = 0;
        if (!this.body) {
          const size = this.prefix.readUInt32BE();
          if (!size || size > MAX_FRAME_BYTES) throw new Error('frame_size');
          this.body = Buffer.alloc(size);
        } else {
          const value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.body));
          this.body = null;
          emit(value);
        }
      }
    } catch (error) { this.failed = true; throw error; }
  }
}

export function validateDelivery(d) {
  exact(d, ['message_id', 'interaction_id', 'sender_id', 'kind', 'body', 'body_sha256']);
  if (![d.message_id, d.interaction_id, d.sender_id].every(id)
      || !['task', 'information'].includes(d.kind) || typeof d.body !== 'string'
      || !d.body.length || Buffer.byteLength(d.body) > 32768
      || !/^[a-f0-9]{64}$/.test(d.body_sha256)
      || createHash('sha256').update(d.body).digest('hex') !== d.body_sha256) throw new Error('invalid_delivery');
  return Object.freeze({ ...d });
}

export function customMessage(delivery, binding) {
  return Object.freeze({
    customType: CUSTOM_TYPE,
    content: `[Wardian peer message; sender=${delivery.sender_id}; message=${delivery.message_id}; interaction=${delivery.interaction_id}; kind=task]\n${delivery.body}`,
    display: true,
    details: Object.freeze({ ...delivery, target_id: binding.target_id, generation: binding.generation,
      session_id: binding.session_id, runtime_nonce: binding.runtime_nonce }),
  });
}
