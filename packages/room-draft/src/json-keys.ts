/** Detect duplicate object members before JSON.parse collapses them. */
export function uniqueJsonKeys(json: string): boolean {
  const containers: (Set<string> | null)[] = [];
  for (let i = 0; i < json.length; i++) {
    const char = json[i];
    if (char === '"') {
      const start = i;
      i++;
      while (i < json.length && json[i] !== '"') {
        if (json[i] === "\\") i++;
        i++;
      }
      if (i >= json.length) return false;
      let next = i + 1;
      while (
        json[next] === " " ||
        json[next] === "\t" ||
        json[next] === "\n" ||
        json[next] === "\r"
      )
        next++;
      if (json[next] === ":") {
        const keys = containers[containers.length - 1];
        if (!keys) return false;
        const key: unknown = JSON.parse(json.slice(start, i + 1));
        if (typeof key !== "string" || keys.has(key)) return false;
        keys.add(key);
      }
    } else if (char === "{" || char === "[") {
      if (containers.length >= 3) return false;
      containers.push(char === "{" ? new Set<string>() : null);
    } else if (char === "}" || char === "]") {
      containers.pop();
    }
  }
  return true;
}
