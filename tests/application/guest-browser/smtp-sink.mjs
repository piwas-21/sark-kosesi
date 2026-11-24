// One fixture-only loopback mailbox. It keeps a single proof in memory.
import { createServer } from "node:net";

const exchange = [
  ["EHLO example.test", "250 fixture"],
  ["MAIL FROM:<sender@example.test>", "250 fixture"],
  ["RCPT TO:<buyer@example.test>", "250 fixture"],
  ["DATA", "354 send data"],
];
const headers = [
  "From: sender@example.test",
  "To: buyer@example.test",
  "Subject: Synthetic guest order link",
  "Content-Type: text/plain; charset=us-ascii",
  "",
];

export async function startGuestLinkSink() {
  const sockets = new Set();
  let connections = 0;
  let proof;
  let invalid = false;
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    const reject = () => {
      invalid = true;
      socket.destroy();
    };
    socket.on("error", reject);
    socket.setTimeout(5000, reject);
    if (++connections !== 1) return reject();

    let pending = "";
    let step = 0;
    let proofLine;
    socket.write("220 fixture\r\n");
    socket.on("data", (chunk) => {
      pending += chunk.toString("latin1");
      if (pending.length > 1024) return reject();
      for (
        let end = pending.indexOf("\r\n");
        end >= 0;
        end = pending.indexOf("\r\n")
      ) {
        const line = pending.slice(0, end);
        pending = pending.slice(end + 2);
        if (step < exchange.length) {
          const [command, reply] = exchange[step++];
          if (line !== command) return reject();
          socket.write(`${reply}\r\n`);
          continue;
        }
        const index = step++ - exchange.length;
        if (index < headers.length) {
          if (line !== headers[index]) return reject();
        } else if (index === headers.length) {
          proofLine = /^proof=([0-9a-f]{64})$/.exec(line)?.[1];
          if (proofLine === undefined) return reject();
        } else if (index === headers.length + 1 && line === ".") {
          proof = proofLine;
          proofLine = undefined;
          socket.write("250 queued\r\n");
        } else if (index === headers.length + 2 && line === "QUIT") {
          socket.end("221 bye\r\n");
        } else {
          return reject();
        }
      }
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const bound = server.address();
  if (!bound || typeof bound === "string")
    throw new Error("No local SMTP sink port");
  return {
    port: bound.port,
    takeProof() {
      if (invalid || connections !== 1 || proof === undefined)
        throw new Error(
          "Expected exactly one accepted synthetic guest-link message",
        );
      const received = proof;
      proof = undefined;
      return received;
    },
    close() {
      for (const socket of sockets) socket.destroy();
      return new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    },
  };
}
