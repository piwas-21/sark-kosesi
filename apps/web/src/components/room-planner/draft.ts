import { measurementNanometres, type MeasurementUnit } from "@piwas/room-fit";
import type { RoomDraft } from "@piwas/room-draft";

export const coordinates = ["width", "depth", "left", "top"] as const;
export type Coordinate = (typeof coordinates)[number];
export type QuarterTurn = 0 | 90 | 180 | 270;
export type ZoneDraft = Record<Coordinate, string> & {
  id: number;
  rotation: QuarterTurn;
};
export const emptyZone = (id: number): ZoneDraft => ({
  id,
  width: "",
  depth: "",
  left: "0",
  top: "0",
  rotation: 0,
});

export function decimal(text: string): string {
  return text
    .trim()
    .replace(/[٠-٩۰-۹]/g, (digit) => String(digit.charCodeAt(0) & 15))
    .replace(/[,٫]/g, ".")
    .replace(/^\.(?=\d)/, "0.")
    .replace(/^0+(?=\d)/, "");
}

export function zoneInput(zone: ZoneDraft) {
  return {
    x: decimal(zone.left),
    y: decimal(zone.top),
    width: decimal(zone.width),
    depth: decimal(zone.depth),
    rotation: zone.rotation,
  };
}

export function badMeasurement(
  value: string,
  unit: MeasurementUnit,
  positive: boolean,
) {
  const measure = measurementNanometres(decimal(value), unit);
  return measure === null || (positive && measure === 0);
}

export function hasMeasurements(zone: ZoneDraft): boolean {
  return Boolean(
    zone.width || zone.depth || zone.left !== "0" || zone.top !== "0",
  );
}

export function draftFileInput(
  unit: MeasurementUnit,
  room: RoomDraft["room"],
  items: ZoneDraft[],
  doorMode: RoomDraft["doorMode"],
  doors: ZoneDraft[],
): RoomDraft {
  const rectangle = ({ width, depth, left, top }: ZoneDraft) => ({
    width: decimal(width),
    depth: decimal(depth),
    left: decimal(left),
    top: decimal(top),
  });
  return {
    unit,
    room: {
      width: decimal(room.width),
      depth: decimal(room.depth),
      tolerance: decimal(room.tolerance),
    },
    items: items.map((zone) => ({
      ...rectangle(zone),
      rotation: zone.rotation,
    })),
    doorMode,
    doors: doors.map(rectangle),
  };
}
