import { describe, expect, it } from "vitest";
import { assessRoomFit } from "../../room-fit/src/index";
import {
  decodeRoomDraft,
  encodeRoomDraft,
  MAX_ROOM_DRAFT_BYTES,
  type RoomDraft,
} from "../src/index";

const piece = {
  width: "150",
  depth: "70",
  left: "280",
  top: "0",
  rotation: 0,
} as const;
const clearance = { width: "20", depth: "30", left: "280", top: "0" };
const draft: RoomDraft = {
  unit: "cm",
  room: { width: "400", depth: "300", tolerance: "0" },
  items: [piece],
  doorMode: "unknown",
  doors: [clearance],
};
const literal =
  '{"format":"piwas-room-plan","version":1,"unit":"cm","room":{"width":"400","depth":"300","tolerance":"0"},"items":[{"width":"150","depth":"70","left":"280","top":"0","rotation":0}],"doorMode":"unknown","doors":[{"width":"20","depth":"30","left":"280","top":"0"}]}';
const invalid = { ok: false, error: "invalid" };
const tooLarge = { ok: false, error: "too-large" };
const wire = () => JSON.parse(literal) as Record<string, unknown>;

function assess(value: RoomDraft) {
  const rectangle = (zone: (typeof value.doors)[number]) => ({
    x: zone.left,
    y: zone.top,
    width: zone.width,
    depth: zone.depth,
  });
  return assessRoomFit({
    unit: value.unit,
    room: value.room,
    tolerance: value.room.tolerance,
    items: value.items.map((zone) => ({
      ...rectangle(zone),
      rotation: zone.rotation,
    })),
    doorClearances:
      value.doorMode === "unknown"
        ? null
        : value.doorMode === "none"
          ? []
          : value.doors.map(rectangle),
  });
}

describe("room draft file", () => {
  it("encodes the exact versioned format and restores hidden clearance drafts", () => {
    expect(encodeRoomDraft(draft)).toEqual({ ok: true, value: literal });
    expect(decodeRoomDraft(literal)).toEqual({ ok: true, value: draft });
    expect(decodeRoomDraft(` \n${literal}\t`)).toEqual({
      ok: true,
      value: draft,
    });
    expect(assess(draft)).toEqual({
      status: "conflict",
      issues: [{ kind: "outside-room", item: 0 }],
    });
    const rotated: RoomDraft = {
      ...draft,
      items: [{ ...piece, rotation: 90 }],
    };
    const encoded = encodeRoomDraft(rotated);
    expect(encoded.ok).toBe(true);
    if (!encoded.ok) throw new Error("expected valid draft");
    const restored = decodeRoomDraft(encoded.value);
    expect(restored).toEqual({ ok: true, value: rotated });
    if (!restored.ok) throw new Error("expected valid file");
    expect(assess(restored.value)).toEqual({
      status: "uncertain",
      reason: "door-clearances-unknown",
    });
    expect(assess({ ...restored.value, doorMode: "none" })).toEqual({
      status: "fit",
    });
  });

  it.each([
    ["mm", "100000", "100000.001"],
    ["cm", "10000", "10000.001"],
    ["in", "3937.007", "3937.008"],
  ] as const)(
    "roundtrips editable %s values at the 100 m boundary",
    (unit, max, over) => {
      const value: RoomDraft = {
        unit,
        room: { width: max, depth: ".", tolerance: "1." },
        items: [
          { width: "", depth: "0.001", left: "0", top: "0.", rotation: 270 },
        ],
        doorMode: "none",
        doors: [{ width: "", depth: "1.234", left: "0.000", top: "." }],
      };
      const encoded = encodeRoomDraft(value);
      expect(encoded.ok).toBe(true);
      if (!encoded.ok) throw new Error("expected valid boundary draft");
      expect(decodeRoomDraft(encoded.value)).toEqual({ ok: true, value });
      expect(
        encodeRoomDraft({ ...value, room: { ...value.room, width: over } }),
      ).toEqual(invalid);
      expect(
        decodeRoomDraft(
          encoded.value.replace(`"width":"${max}"`, `"width":"${over}"`),
        ),
      ).toEqual(invalid);
    },
  );

  it("preserves all clearance modes, quarter turns, empty lists and maximum counts", () => {
    for (const doorMode of ["unknown", "none", "measured"] as const) {
      const rotations = [0, 90, 180, 270] as const;
      const value: RoomDraft = {
        ...draft,
        doorMode,
        items: Array.from({ length: 50 }, (_, index) => ({
          width: "1",
          depth: "1",
          left: "0",
          top: "0",
          rotation: rotations[index % 4] ?? 0,
        })),
        doors: Array.from({ length: 20 }, () => ({
          width: "1",
          depth: "1",
          left: "0",
          top: "0",
        })),
      };
      const encoded = encodeRoomDraft(value);
      expect(encoded.ok).toBe(true);
      if (!encoded.ok) throw new Error("expected bounded draft");
      expect(
        new TextEncoder().encode(encoded.value).length,
      ).toBeLessThanOrEqual(MAX_ROOM_DRAFT_BYTES);
      expect(decodeRoomDraft(encoded.value)).toEqual({ ok: true, value });
      expect(
        encodeRoomDraft({ ...value, items: [...value.items, piece] }),
      ).toEqual(invalid);
      expect(
        encodeRoomDraft({ ...value, doors: [...value.doors, clearance] }),
      ).toEqual(invalid);
      expect(
        decodeRoomDraft(
          JSON.stringify({ ...wire(), items: [...value.items, piece] }),
        ),
      ).toEqual(invalid);
      expect(
        decodeRoomDraft(
          JSON.stringify({ ...wire(), doors: [...value.doors, clearance] }),
        ),
      ).toEqual(invalid);
    }
    const empty: RoomDraft = {
      ...draft,
      items: [],
      doors: [],
      doorMode: "measured",
    };
    const encoded = encodeRoomDraft(empty);
    expect(encoded.ok).toBe(true);
    if (!encoded.ok) throw new Error("expected empty draft");
    expect(decodeRoomDraft(encoded.value)).toEqual({ ok: true, value: empty });
  });

  it.each([
    "-1",
    "+1",
    "1e2",
    "01",
    ".5",
    "1.0000",
    " 1",
    "1,2",
    "NaN",
    "Infinity",
    "١",
    "<script>",
  ])("rejects noncanonical measurement %s on both paths", (width) => {
    expect(
      encodeRoomDraft({ ...draft, room: { ...draft.room, width } }),
    ).toEqual(invalid);
    expect(
      decodeRoomDraft(
        JSON.stringify({ ...wire(), room: { ...draft.room, width } }),
      ),
    ).toEqual(invalid);
  });

  it("rejects wrong types, versions, keys, counts, modes and rotation", () => {
    const cases: unknown[] = [
      null,
      [],
      "text",
      1,
      { ...wire(), format: "other" },
      { ...wire(), version: 2 },
      { ...wire(), version: "1" },
      { ...wire(), unit: "ft" },
      { ...wire(), doorMode: "open" },
      { ...wire(), items: {} },
      { ...wire(), doors: null },
      { ...wire(), room: { ...draft.room, width: 400 } },
      { ...wire(), room: { ...draft.room, extra: true } },
      { ...wire(), items: [{ ...piece, rotation: 45 }] },
      { ...wire(), items: [{ ...piece, note: "hidden" }] },
      { ...wire(), doors: [{ ...clearance, rotation: 0 }] },
      { ...wire(), secret: "ignored?" },
      { ...wire(), room: { width: "1", depth: "2" } },
    ];
    for (const value of cases)
      expect(decodeRoomDraft(JSON.stringify(value))).toEqual(invalid);
    expect(
      decodeRoomDraft(literal.replace('"format":', '"__proto__":{},"format":')),
    ).toEqual(invalid);
    expect(
      decodeRoomDraft(
        literal.replace('"format":', '"constructor":{},"format":'),
      ),
    ).toEqual(invalid);
    expect(decodeRoomDraft("{")).toEqual(invalid);
    expect(decodeRoomDraft("[".repeat(100) + "]".repeat(100))).toEqual(invalid);
    expect(encodeRoomDraft({ ...draft, extra: true })).toEqual(invalid);
    expect(
      encodeRoomDraft({
        ...draft,
        items: [{ ...piece, rotation: 45 }],
      }),
    ).toEqual(invalid);
  });

  it("rejects repeated JSON keys at every object level, including escaped equivalents", () => {
    const duplicates = [
      literal.replace('"unit":"cm"', '"unit":"cm","unit":"in"'),
      literal.replace('"unit":"cm"', '"unit":"cm","\\u0075nit":"cm"'),
      literal.replace('"width":"400"', '"width":"400","width":"400"'),
      literal.replace('"width":"400"', '"width":"400","\\u0077idth":"401"'),
      literal.replace('"rotation":0', '"rotation":0,"rotation":90'),
      literal.replace('"rotation":0', '"rotation":0,"\\u0072otation":0'),
      literal.replace('"width":"20"', '"width":"20","width":"21"'),
    ];
    for (const json of duplicates)
      expect(decodeRoomDraft(json)).toEqual(invalid);
    const escaped = literal
      .replace('"unit":"cm"', '"\\u0075nit":"\\u0063m"')
      .replace('"width":"400"', '"width":"\\u003400"');
    expect(decodeRoomDraft(escaped)).toEqual({ ok: true, value: draft });
  });

  it("enforces the UTF-8 byte limit before parsing", () => {
    expect(MAX_ROOM_DRAFT_BYTES).toBe(16_384);
    expect(decodeRoomDraft(" ".repeat(MAX_ROOM_DRAFT_BYTES))).toEqual(invalid);
    expect(decodeRoomDraft(" ".repeat(MAX_ROOM_DRAFT_BYTES + 1))).toEqual(
      tooLarge,
    );
    expect(decodeRoomDraft("é".repeat(MAX_ROOM_DRAFT_BYTES / 2))).toEqual(
      invalid,
    );
    expect(decodeRoomDraft("é".repeat(MAX_ROOM_DRAFT_BYTES / 2 + 1))).toEqual(
      tooLarge,
    );
  });

  it("rejects hostile in-memory objects without invoking their accessors", () => {
    let calls = 0;
    const room = Object.defineProperty({ ...draft.room }, "width", {
      enumerable: true,
      get() {
        calls += 1;
        return "400";
      },
    });
    expect(encodeRoomDraft({ ...draft, room })).toEqual(invalid);
    expect(calls).toBe(0);
    const inherited = Object.assign(Object.create({ untrusted: true }), draft);
    expect(encodeRoomDraft(inherited)).toEqual(invalid);
    const symbolic = { ...draft, [Symbol("hidden")]: true };
    expect(encodeRoomDraft(symbolic)).toEqual(invalid);
    const sparse = Array<RoomDraft["items"][number]>(1);
    expect(encodeRoomDraft({ ...draft, items: sparse })).toEqual(invalid);
  });

  it("encodes data descriptor snapshots without ordinary proxy property reads", () => {
    let gets = 0;
    const proxied = new Proxy(draft, {
      get(target, key, receiver) {
        gets += 1;
        return key === "unit" ? "in" : Reflect.get(target, key, receiver);
      },
    });
    expect(encodeRoomDraft(proxied)).toEqual({ ok: true, value: literal });
    expect(gets).toBe(0);
  });
});
