"""Reject empty, stale and wrong-scope Sonar results before believing zero issues."""

PROJECT = "piwas-sark-kosesi"


def validate_analysis(component, analyses, analysis_id, head, gate, measures, issues, hotspots):
    if component.get("component", {}).get("key") != PROJECT:
        raise ValueError("Sonar project identity was not established")
    records = analyses.get("analyses", [])
    if not records or records[0].get("key") != analysis_id or records[0].get("revision") != head:
        raise ValueError("Sonar analysis does not match current task and revision")
    status = gate.get("projectStatus", {})
    if status.get("status") != "OK" or not status.get("conditions"):
        raise ValueError("Sonar quality gate is failing or has no evaluated conditions")
    if any(c.get("status") != "OK" for c in status["conditions"]):
        raise ValueError("Sonar quality condition did not pass")
    source = measures.get("component", {})
    metrics = {m["metric"]: m.get("value") for m in source.get("measures", [])}
    if source.get("key") != PROJECT or int(metrics.get("ncloc", 0)) <= 0:
        raise ValueError("Sonar source analysis is empty")
    if (
        issues.get("errors")
        or issues.get("paging", {}).get("total") != 0
        or issues.get("issues") != []
    ):
        raise ValueError("Sonar unresolved issue result is missing or not empty")
    if (
        hotspots.get("errors")
        or hotspots.get("paging", {}).get("total") != 0
        or hotspots.get("hotspots") != []
    ):
        raise ValueError("Sonar outstanding hotspot result is missing or not empty")
    return {
        "schemaVersion": 1,
        "project": PROJECT,
        "revision": head,
        "analysisId": analysis_id,
        "ncloc": int(metrics["ncloc"]),
        "conditions": status["conditions"],
        "issues": 0,
        "hotspots": 0,
    }


def validate_evidence(evidence, head):
    if evidence.get("schemaVersion") != 1 or evidence.get("project") != PROJECT:
        raise ValueError("Wrong Sonar evidence scope")
    if evidence.get("revision") != head or not evidence.get("analysisId"):
        raise ValueError("Sonar evidence is stale or analysis identity missing")
    if not isinstance(evidence.get("ncloc"), int) or evidence["ncloc"] <= 0:
        raise ValueError("Sonar evidence contains no analyzed source")
    if evidence.get("issues") != 0 or evidence.get("hotspots") != 0:
        raise ValueError("Sonar evidence contains unresolved findings")
    if not evidence.get("conditions") or any(
        c.get("status") != "OK" for c in evidence["conditions"]
    ):
        raise ValueError("Sonar evidence has no passing evaluated quality conditions")
