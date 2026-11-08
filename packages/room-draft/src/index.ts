import { measurementNanometres, type MeasurementUnit } from "@piwas/room-fit";
import { uniqueJsonKeys } from "./json-keys";

/** A draft is local editable input, not a fit assessment or product specification. */
export type RoomDraft = {
  unit: MeasurementUnit;
  room: { width: string; depth: string; tolerance: string };
  items: {
    width: string;
    depth: string;
    left: string;
    top: string;
    rotation: 0 | 90 | 180 | 270;
  }[];
  doorMode: "unknown" | "none" | "measured";
  doors: { width: string; depth: string; left: string; top: string }[];
};

export type DraftResult<T> =
  { ok: true; value: T } | { ok: false; error: "too-large" | "invalid" };

export const MAX_ROOM_DRAFT_BYTES = 16_384;
const FORMAT = "piwas-room-plan";
const invalid = Object.freeze({
  ok: false,
  error: "invalid",
} satisfies DraftResult<never>);
const tooLarge = Object.freeze({
  ok: false,
  error: "too-large",
} satisfies DraftResult<never>);

function closedRecord(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return null;
  const prototype: unknown = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return null;
  if (Reflect.ownKeys(value).length !== keys.length) return null;
  const snapshot: Record<string, unknown> = Object.create(null);
  for (const key of keys) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !descriptor.enumerable || !("value" in descriptor))
      return null;
    snapshot[key] = descriptor.value;
  }
  return snapshot;
}

function arrayValues(value: unknown, maximum: number): unknown[] | null {
  if (!Array.isArray(value) || Object.getPrototypeOf(value) !== Array.prototype)
    return null;
  const lengthDescriptor = Object.getOwnPropertyDescriptor(value, "length");
  if (!lengthDescriptor || !("value" in lengthDescriptor)) return null;
  const length: unknown = lengthDescriptor.value;
  if (
    typeof length !== "number" ||
    !Number.isSafeInteger(length) ||
    length < 0 ||
    length > maximum ||
    Reflect.ownKeys(value).length !== length + 1
  )
    return null;
  const result: unknown[] = [];
  for (let index = 0; index < length; index++) {
    const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
    if (!descriptor || !descriptor.enumerable || !("value" in descriptor))
      return null;
    result.push(descriptor.value);
  }
  return result;
}

function measurement(value: unknown, unit: MeasurementUnit): value is string {
  if (typeof value !== "string" || value.length > 10) return false;
  if (value === "" || value === ".") return true;
  if (value.endsWith(".")) {
    return measurementNanometres(value.slice(0, -1), unit) !== null;
  }
  return measurementNanometres(value, unit) !== null;
}

function rectangle(value: unknown, unit: MeasurementUnit) {
  const record = closedRecord(value, ["width", "depth", "left", "top"]);
  if (!record) return null;
  const { width, depth, left, top } = record;
  if (
    !measurement(width, unit) ||
    !measurement(depth, unit) ||
    !measurement(left, unit) ||
    !measurement(top, unit)
  )
    return null;
  return { width, depth, left, top };
}

function parseDraft(value: unknown, wire: boolean): RoomDraft | null {
  const keys = ["unit", "room", "items", "doorMode", "doors"];
  const record = closedRecord(
    value,
    wire ? ["format", "version", ...keys] : keys,
  );
  if (!record) return null;
  if (wire && (record.format !== FORMAT || record.version !== 1)) return null;
  const { unit, room, doorMode } = record;
  if (
    (unit !== "mm" && unit !== "cm" && unit !== "in") ||
    (doorMode !== "unknown" && doorMode !== "none" && doorMode !== "measured")
  )
    return null;
  const roomRecord = closedRecord(room, ["width", "depth", "tolerance"]);
  if (!roomRecord) return null;
  const { width, depth, tolerance } = roomRecord;
  if (
    !measurement(width, unit) ||
    !measurement(depth, unit) ||
    !measurement(tolerance, unit)
  )
    return null;
  const itemValues = arrayValues(record.items, 50);
  const doorValues = arrayValues(record.doors, 20);
  if (itemValues === null || doorValues === null) return null;
  const items: RoomDraft["items"] = [];
  for (const entry of itemValues) {
    const item = closedRecord(entry, [
      "width",
      "depth",
      "left",
      "top",
      "rotation",
    ]);
    if (!item) return null;
    const { width, depth, left, top, rotation } = item;
    if (
      !measurement(width, unit) ||
      !measurement(depth, unit) ||
      !measurement(left, unit) ||
      !measurement(top, unit) ||
      (rotation !== 0 &&
        rotation !== 90 &&
        rotation !== 180 &&
        rotation !== 270)
    )
      return null;
    items.push({ width, depth, left, top, rotation });
  }
  const doors: RoomDraft["doors"] = [];
  for (const entry of doorValues) {
    const door = rectangle(entry, unit);
    if (door === null) return null;
    doors.push(door);
  }
  return { unit, room: { width, depth, tolerance }, items, doorMode, doors };
}

function exceedsLimit(value: string): boolean {
  return (
    value.length > MAX_ROOM_DRAFT_BYTES ||
    new TextEncoder().encode(value).length > MAX_ROOM_DRAFT_BYTES
  );
}

export function encodeRoomDraft(draft: unknown): DraftResult<string> {
  try {
    const parsed = parseDraft(draft, false);
    if (parsed === null) return invalid;
    const json = JSON.stringify({ format: FORMAT, version: 1, ...parsed });
    return exceedsLimit(json) ? tooLarge : { ok: true, value: json };
  } catch {
    return invalid;
  }
}

export function decodeRoomDraft(json: string): DraftResult<RoomDraft> {
  if (typeof json !== "string") return invalid;
  if (exceedsLimit(json)) return tooLarge;
  try {
    if (!uniqueJsonKeys(json)) return invalid;
    const value: unknown = JSON.parse(json);
    const parsed = parseDraft(value, true);
    return parsed === null ? invalid : { ok: true, value: parsed };
  } catch {
    return invalid;
  }
}
