import { notFound } from "next/navigation";
import { plannerCopy } from "../../../components/room-planner/copy";
import { RoomPlanner } from "../../../components/room-planner/planner";
import { isLocale } from "../../../lib/locale";

export default async function RoomPlannerPage({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  if (!isLocale(locale)) notFound();
  return <RoomPlanner copy={plannerCopy[locale]} />;
}
