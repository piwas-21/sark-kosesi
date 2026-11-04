#!/usr/bin/env python3
"""Generate the openapi-fetch view of the supported Rust OpenAPI subset.

Hey API owns schema and parameter type generation. This adapter preserves wire
metadata Hey API flattens, and rejects unsupported shapes instead of widening.
"""

import json
import re
from pathlib import Path

NAME = re.compile(r"[A-Z][A-Za-z0-9]*\Z")
OPERATION = re.compile(r"[a-z][a-z0-9]*(?:_[a-z0-9]+)*\Z")
METHODS = {"get", "put", "post", "delete", "options", "head", "patch", "trace"}


def keys(value, allowed, label):
    if not isinstance(value, dict) or set(value) - allowed:
        raise ValueError(f"Unsupported {label} fields")


def ref_type(schema, names):
    keys(schema, {"$ref"}, "body schema")
    ref = schema.get("$ref", "")
    prefix = "#/components/schemas/"
    name = ref.removeprefix(prefix)
    if not ref.startswith(prefix) or name not in names:
        raise ValueError("Body schema must reference a named schema")
    return f"Model.{name}"


def content_types(content, names):
    if not isinstance(content, dict) or not content:
        raise ValueError("Content must be a nonempty media type map")
    result = []
    for media, definition in content.items():
        keys(definition, {"schema"}, "media type")
        if "schema" not in definition or not isinstance(media, str) or not media:
            raise ValueError("Media type requires a schema")
        result.append(f"{json.dumps(media)}: {ref_type(definition['schema'], names)};")
    return " ".join(result)


def generate(document):
    keys(document, {"openapi", "info", "paths", "components"}, "document")
    if document.get("openapi") != "3.1.0":
        raise ValueError("Expected OpenAPI 3.1.0")
    components = document.get("components")
    keys(components, {"schemas", "securitySchemes"}, "components")
    schemas = components.get("schemas")
    if not isinstance(schemas, dict) or any(not NAME.fullmatch(k) for k in schemas):
        raise ValueError("Unsupported schema name")
    names = set(schemas)
    lines = [
        "// Generated from Rust OpenAPI and Hey API types. Do not edit.",
        'import type * as Model from "./models.gen";',
        "",
        "export interface components {",
        "  schemas: {",
    ]
    lines.extend(f"    {name}: Model.{name};" for name in schemas)
    lines.extend(["  };", "}", "", "export interface paths {"])
    paths = document.get("paths")
    if not isinstance(paths, dict) or not paths:
        raise ValueError("Expected nonempty paths")
    operations = []
    seen = set()
    for path, methods in paths.items():
        if not isinstance(path, str) or not path.startswith("/") or not isinstance(methods, dict):
            raise ValueError("Unsupported path")
        if not methods or set(methods) - METHODS:
            raise ValueError("Unsupported path-level field or method")
        lines.append(f"  {json.dumps(path)}: {{")
        for method, operation in methods.items():
            keys(
                operation,
                {
                    "operationId",
                    "parameters",
                    "requestBody",
                    "responses",
                    "security",
                    "tags",
                    "summary",
                    "description",
                },
                "operation",
            )
            op = operation.get("operationId", "")
            if not OPERATION.fullmatch(op) or op in seen:
                raise ValueError("Operation IDs must be unique snake_case identifiers")
            seen.add(op)
            typename = "".join(word.capitalize() for word in op.split("_"))
            lines.append(f"    {method}: operations[{json.dumps(op)}];")
            operations.append((op, typename, operation))
        lines.append("  };")
    lines.extend(["}", "", "export interface operations {"])
    for op, typename, operation in operations:
        lines.extend([f"  {json.dumps(op)}: {{", "    parameters: {"])
        grouped = {k: [] for k in ("query", "header", "path", "cookie")}
        parameters = operation.get("parameters", [])
        if not isinstance(parameters, list):
            raise ValueError("Unsupported parameters")
        for parameter in parameters:
            keys(parameter, {"name", "in", "required", "schema", "description"}, "parameter")
            location = parameter.get("in")
            if location not in ("query", "header", "path"):
                raise ValueError("Unsupported parameter location")
            if (
                not isinstance(parameter.get("name"), str)
                or not isinstance(parameter.get("required"), bool)
                or "schema" not in parameter
            ):
                raise ValueError("Invalid parameter")
            grouped[location].append(parameter)
        for location, entries in grouped.items():
            if entries:
                field = "headers" if location == "header" else location
                required = any(p["required"] for p in entries)
                lines.append(
                    f"      {location}{'' if required else '?'}: NonNullable<Model.{typename}Data[{json.dumps(field)}]>;"
                )
            else:
                lines.append(f"      {location}?: never;")
        lines.append("    };")
        body = operation.get("requestBody")
        if body is None:
            lines.append("    requestBody?: never;")
        else:
            keys(body, {"content", "required", "description"}, "request body")
            if not isinstance(body.get("required"), bool):
                raise ValueError("Request body requires an explicit required flag")
            content = content_types(body.get("content"), names)
            lines.append(
                f"    requestBody{'' if body['required'] else '?'}: {{ content: {{ {content} }} }};"
            )
        responses = operation.get("responses")
        if not isinstance(responses, dict) or not responses:
            raise ValueError("Expected responses")
        lines.append("    responses: {")
        for status, response in responses.items():
            if not re.fullmatch(r"[1-5][0-9]{2}", status):
                raise ValueError("Unsupported response status")
            keys(response, {"description", "content"}, "response")
            if "content" in response:
                content = content_types(response["content"], names)
                lines.append(
                    f"      {status}: {{ headers: {{ [name: string]: unknown }}; content: {{ {content} }} }};"
                )
            else:
                lines.append(
                    f"      {status}: {{ headers: {{ [name: string]: unknown }}; content?: never }};"
                )
        lines.extend(["    };", "  };"])
    lines.extend(["}", ""])
    return "\n".join(lines)


def main():
    import sys

    document = json.loads(Path(sys.argv[1]).read_text())
    Path(sys.argv[2]).write_text(generate(document))


if __name__ == "__main__":
    main()
