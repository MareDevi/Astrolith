import { appendFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { glob } from "astro/loaders";

function oracleOutputPath() {
  const output = process.env.ASTROLITH_ORACLE_OUTPUT;
  if (!output) throw new Error("ASTROLITH_ORACLE_OUTPUT is required by the fixture oracle");
  return output;
}

function projectRelativePath(filePath) {
  if (typeof filePath !== "string" || filePath.length === 0) {
    throw new Error("Astro glob loader store.set emitted an entry without filePath");
  }
  const absolute = filePath.startsWith("file:") ? fileURLToPath(filePath) : resolve(filePath);
  return relative(process.cwd(), absolute).replaceAll("\\", "/");
}

export function recordingGlob(options) {
  const delegate = glob(options);
  return {
    ...delegate,
    async load(context) {
      const store = new Proxy(context.store, {
        get(target, property, receiver) {
          const value = Reflect.get(target, property, receiver);
          if (property !== "set") return value;
          return function recordingSet(entry, ...rest) {
            appendFileSync(oracleOutputPath(), `${JSON.stringify({
              collection: context.collection,
              id: entry.id,
              filePath: projectRelativePath(entry.filePath),
            })}\n`);
            return Reflect.apply(value, target, [entry, ...rest]);
          };
        },
      });
      const delegatedContext = new Proxy(context, {
        get(target, property, receiver) {
          return property === "store" ? store : Reflect.get(target, property, receiver);
        },
      });
      return delegate.load.call(delegate, delegatedContext);
    },
  };
}
