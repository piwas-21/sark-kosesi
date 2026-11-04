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
  emptyZone,
  hasMeasurements,
  zoneInput,
  type ZoneDraft,
} from "./draft";
import { PlanSheet } from "./plan-sheet";
import { MeasurementField, ZoneFields } from "./zone-fields";
import styles from "./planner.module.css";

type DoorState = "unknown" | "none" | "measured";
const roomFields = ["width", "depth", "tolerance"] as const;

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
  const showErrors = result?.status === "invalid";

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
                  doorState === "unknown" ? null : activeDoors.map(zoneInput),
              }),
            );
          }}
        >
          <section className={styles.section} aria-labelledby="plan-room">
            <h2 id="plan-room">{copy.room}</h2>
            <label className={styles.control}>
              {copy.unit}
              <select
                id="plan-unit"
                value={unit}
                disabled={locked}
                aria-describedby="plan-unit-help"
                onChange={(event) => {
                  const value = event.target.value;
                  if (value === "mm" || value === "cm" || value === "in")
                    setUnit(value);
                }}
              >
                <option value="mm">{copy.millimetres}</option>
                <option value="cm">{copy.centimetres}</option>
                <option value="in">{copy.inches}</option>
              </select>
            </label>
            <p id="plan-unit-help" className={styles.note}>
              {copy.unitHelp}
            </p>
            <div className={styles.fields}>
              {roomFields.map((key) => (
                <MeasurementField
                  key={key}
                  id={`plan-room-${key}`}
                  label={copy[key]}
                  value={room[key]}
                  unit={unit}
                  positive={key !== "tolerance"}
                  error={showErrors ? copy.invalid : null}
                  onChange={(value) => setRoom({ ...room, [key]: value })}
                />
              ))}
            </div>
            <p className={styles.note}>{copy.toleranceHelp}</p>
          </section>

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
