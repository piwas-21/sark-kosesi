import { describe, expect, it } from "vitest";
import { assessRoomFit, measurementNanometres } from "../src/index";

const room = { width: "500", depth: "400" };
const item = { x: "20", y: "20", width: "100", depth: "60", rotation: 0 };
const plan = {
  unit: "cm",
  room,
  tolerance: "0",
  items: [item],
  doorClearances: [],
};

describe("measurement conversion", () => {
  it("converts decimals to exact integer nanometres", () => {
    expect(measurementNanometres("1.001", "mm")).toBe(1_001_000);
    expect(measurementNanometres("1.25", "cm")).toBe(12_500_000);
    expect(measurementNanometres("1.001", "in")).toBe(25_425_400);
    expect(measurementNanometres("100000", "mm")).toBe(100_000_000_000);
    expect(measurementNanometres("3937.007", "in")).toBe(99_999_977_800);
    expect(measurementNanometres("999999.999", "in")).toBeNull();
  });
  it.each(["", "-1", "+1", "1e2", "01", ".5", "1.", "1.0000", "100000.001"])(
    "rejects invalid or out-of-range measurement %s",
    (value) => expect(measurementNanometres(value, "mm")).toBeNull(),
  );
  it("rejects unknown units and numeric input", () => {
    expect(measurementNanometres("1", "ft")).toBeNull();
    expect(measurementNanometres(1, "mm")).toBeNull();
  });
});

describe("room fit", () => {
  it("accepts a measured plan and permits exact edge contact", () => {
    expect(assessRoomFit(plan)).toEqual({ status: "fit" });
    expect(
      assessRoomFit({ ...plan, items: [{ ...item, x: "400", y: "340" }] }),
    ).toEqual({ status: "fit" });
  });
  it("rotates around the fixed top-left at each quarter turn", () => {
    const placed = { ...item, x: "420", y: "290" };
    expect(
      assessRoomFit({ ...plan, items: [{ ...placed, rotation: 0 }] }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "outside-room", item: 0 }],
    });
    expect(
      assessRoomFit({ ...plan, items: [{ ...placed, rotation: 90 }] }),
    ).toEqual({ status: "fit" });
    expect(
      assessRoomFit({ ...plan, items: [{ ...placed, rotation: 180 }] }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "outside-room", item: 0 }],
    });
    expect(
      assessRoomFit({ ...plan, items: [{ ...placed, rotation: 270 }] }),
    ).toEqual({ status: "fit" });
  });
  it("uses positive area for item and door intersections", () => {
    const second = { ...item, x: "120" };
    const door = { x: "20", y: "80", width: "100", depth: "20" };
    expect(
      assessRoomFit({ ...plan, items: [item, second], doorClearances: [door] }),
    ).toEqual({ status: "fit" });
    expect(
      assessRoomFit({
        ...plan,
        items: [item, { ...second, x: "119.999" }],
        doorClearances: [{ ...door, y: "79.999" }],
      }),
    ).toEqual({
      status: "conflict",
      issues: [
        { kind: "door-clearance", item: 0, door: 0 },
        { kind: "item-overlap", item: 1, other: 0 },
        { kind: "door-clearance", item: 1, door: 0 },
      ],
    });
  });
  it("shrinks the room and expands every object by the tolerance", () => {
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        items: [{ ...item, x: "10", y: "10" }],
      }),
    ).toEqual({ status: "fit" });
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        items: [{ ...item, x: "9.999", y: "10" }],
      }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "outside-room", item: 0 }],
    });
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        items: [item, { ...item, x: "130" }],
      }),
    ).toEqual({ status: "fit" });
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        items: [item, { ...item, x: "129.999" }],
      }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "item-overlap", item: 1, other: 0 }],
    });
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        doorClearances: [{ x: "130", y: "20", width: "20", depth: "60" }],
      }),
    ).toEqual({ status: "fit" });
    expect(
      assessRoomFit({
        ...plan,
        tolerance: "5",
        doorClearances: [{ x: "129.999", y: "20", width: "20", depth: "60" }],
      }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "door-clearance", item: 0, door: 0 }],
    });
  });
  it("distinguishes unknown clearance and unassessable room from a fit", () => {
    expect(assessRoomFit({ ...plan, doorClearances: null })).toEqual({
      status: "uncertain",
      reason: "door-clearances-unknown",
    });
    expect(
      assessRoomFit({
        ...plan,
        doorClearances: null,
        items: [{ ...item, x: "499" }],
      }),
    ).toEqual({
      status: "conflict",
      issues: [{ kind: "outside-room", item: 0 }],
    });
    expect(assessRoomFit({ ...plan, items: [] })).toEqual({
      status: "uncertain",
      reason: "no-items",
    });
    expect(assessRoomFit({ ...plan, tolerance: "200" })).toEqual({
      status: "uncertain",
      reason: "room-tolerance",
    });
  });
  it("rejects malformed, missing and unbounded data", () => {
    expect(assessRoomFit(null)).toEqual({ status: "invalid" });
    expect(assessRoomFit({ ...plan, doorClearances: undefined })).toEqual({
      status: "invalid",
    });
    expect(
      assessRoomFit({ ...plan, room: { width: "0", depth: "1" } }),
    ).toEqual({ status: "invalid" });
    expect(
      assessRoomFit({ ...plan, items: [{ ...item, rotation: 45 }] }),
    ).toEqual({ status: "invalid" });
    expect(
      assessRoomFit({ ...plan, items: [{ ...item, width: "0" }] }),
    ).toEqual({ status: "invalid" });
    expect(assessRoomFit({ ...plan, items: Array(51).fill(item) })).toEqual({
      status: "invalid",
    });
    expect(
      assessRoomFit({ ...plan, doorClearances: Array(21).fill(item) }),
    ).toEqual({ status: "invalid" });
  });
});
