"use client";

import { useRef, useState } from "react";
import { Button } from "@piwas/design";
import {
  assessRoomFit,
  type FitResult,
  type MeasurementUnit,
} from "@piwas/room-fit";
import type { PlannerCopy } from "./copy";
import {
  decimal,
  draftFileInput,
  emptyZone,
  hasMeasurements,
  zoneInput,
  type ZoneDraft,
} from "./draft";
import { DraftFiles } from "./draft-files";
import { PlanSheet } from "./plan-sheet";
import { RoomFields, ZoneFields } from "./zone-fields";
import styles from "./planner.module.css";

type DoorState = "unknown" | "none" | "measured";

export function RoomPlanner({ copy }: { copy: PlannerCopy }) {
  const [unit, setUnit] = useState<MeasurementUnit>("cm");
  const [room, setRoom] = useState({ width: "", depth: "", tolerance: "0" });
  const [items, setItems] = useState([emptyZone(1)]);
  const [doors, setDoors] = useState([emptyZone(2)]);
  const [doorState, setDoorState] = useState<DoorState>("unknown");
  const [result, setResult] = useState<FitResult | null>(null);
  const nextId = useRef(3);
  const locked = Boolean(
    room.width ||
    room.depth ||
    room.tolerance !== "0" ||
    items.some(hasMeasurements) ||
    doors.some(hasMeasurements),
  );
  const activeDoors = doorState === "measured" ? doors : [];
  const [showErrors, setShowErrors] = useState(false);

  function changeZone(
    collection: "items" | "doors",
    id: number,
    change: Partial<Omit<ZoneDraft, "id">>,
  ) {
    const setZones = collection === "items" ? setItems : setDoors;
    setZones((zones) =>
      zones.map((zone) => (zone.id === id ? { ...zone, ...change } : zone)),
    );
  }
  function addZone(collection: "items" | "doors") {
    const zone = emptyZone(nextId.current++);
    const setZones = collection === "items" ? setItems : setDoors;
    setZones((zones) => [...zones, zone]);
    setResult(null);
    requestAnimationFrame(() =>
      document.getElementById(`plan-${collection}-${zone.id}-width`)?.focus(),
    );
  }
  function removeZone(collection: "items" | "doors", id: number) {
    const setZones = collection === "items" ? setItems : setDoors;
    setZones((zones) => zones.filter((zone) => zone.id !== id));
    setResult(null);
    requestAnimationFrame(() =>
      document.getElementById(`plan-add-${collection}`)?.focus(),
    );
  }
  function clear() {
    setRoom({ width: "", depth: "", tolerance: "0" });
    setItems([emptyZone(nextId.current++)]);
    setDoors([emptyZone(nextId.current++)]);
    setDoorState("unknown");
    setShowErrors(false);
    setResult(null);
    requestAnimationFrame(() => document.getElementById("plan-unit")?.focus());
  }

  return (
    <div>
      <div className={styles.heading}>
        <h1>{copy.title}</h1>
        <p>{copy.intro}</p>
        <p className={styles.note}>{copy.privacy}</p>
      </div>
      <div className={styles.workbench}>
        <form
          noValidate
          onChange={() => setResult(null)}
          onSubmit={(event) => {
            event.preventDefault();
            setShowErrors(true);
            setResult(
              assessRoomFit({
                unit,
                room: {
                  width: decimal(room.width),
                  depth: decimal(room.depth),
                },
                tolerance: decimal(room.tolerance),
                items: items.map(zoneInput),
                doorClearances:
                  doorState === "unknown" ||
                  (doorState === "measured" && activeDoors.length === 0)
                    ? null
                    : activeDoors.map(zoneInput),
              }),
            );
          }}
        >
          <RoomFields
            copy={copy}
            unit={unit}
            locked={locked}
            room={room}
            showErrors={showErrors}
            onUnit={setUnit}
            onMeasurement={(key, value) => setRoom({ ...room, [key]: value })}
          />

          <section className={styles.section} aria-labelledby="plan-furniture">
            <h2 id="plan-furniture">{copy.pieces}</h2>
            <p>{copy.placement}</p>
            {items.map((zone, index) => (
              <ZoneFields
                key={zone.id}
                copy={copy}
                prefix="plan-items"
                label={`${copy.piece} ${index + 1}`}
                unit={unit}
                zone={zone}
                rotate
                showErrors={showErrors}
                onCoordinate={(key, value) =>
                  changeZone("items", zone.id, { [key]: value })
                }
                onRotation={(rotation) =>
                  changeZone("items", zone.id, { rotation })
                }
                onRemove={() => removeZone("items", zone.id)}
              />
            ))}
            <button
              id="plan-add-items"
              type="button"
              className={styles.textAction}
              disabled={items.length >= 50}
              onClick={() => addZone("items")}
            >
              {copy.addPiece}
            </button>
          </section>

          <section className={styles.section} aria-labelledby="plan-doors">
            <h2 id="plan-doors">{copy.doors}</h2>
            <label className={styles.control}>
              {copy.doorStatus}
              <select
                value={doorState}
                onChange={(event) => {
                  const value = event.target.value;
                  if (
                    value === "unknown" ||
                    value === "none" ||
                    value === "measured"
                  )
                    setDoorState(value);
                }}
              >
                <option value="unknown">{copy.unknown}</option>
                <option value="none">{copy.none}</option>
                <option value="measured">{copy.measured}</option>
              </select>
            </label>
            {activeDoors.map((zone, index) => (
              <ZoneFields
                key={zone.id}
                copy={copy}
                prefix="plan-doors"
                label={`${copy.door} ${index + 1}`}
                unit={unit}
                zone={zone}
                rotate={false}
                showErrors={showErrors}
                onCoordinate={(key, value) =>
                  changeZone("doors", zone.id, { [key]: value })
                }
                {...(doors.length > 1
                  ? { onRemove: () => removeZone("doors", zone.id) }
                  : {})}
              />
            ))}
            {doorState === "measured" && (
              <button
                id="plan-add-doors"
                type="button"
                className={styles.textAction}
                disabled={doors.length >= 20}
                onClick={() => addZone("doors")}
              >
                {copy.addDoor}
              </button>
            )}
          </section>
          <div className={styles.actions}>
            <Button type="submit">{copy.check}</Button>
            <button type="button" className={styles.textAction} onClick={clear}>
              {copy.clear}
            </button>
          </div>
          <DraftFiles
            copy={copy.files}
            draft={draftFileInput(unit, room, items, doorState, doors)}
            onApply={(opened) => {
              setUnit(opened.unit);
              setRoom(opened.room);
              setItems(
                opened.items.map((zone) => ({ ...zone, id: nextId.current++ })),
              );
              setDoors(
                opened.doors.map((zone) => ({
                  ...zone,
                  id: nextId.current++,
                  rotation: 0,
                })),
              );
              setDoorState(opened.doorMode);
              setShowErrors(false);
              setResult(null);
            }}
          />
        </form>
        <PlanSheet
          copy={copy}
          unit={unit}
          width={room.width}
          depth={room.depth}
          items={items}
          doors={activeDoors}
          result={result}
        />
      </div>
    </div>
  );
}
