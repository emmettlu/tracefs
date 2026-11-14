import { connect } from "node:net";

import type { TracefsProtocol, Request, Response } from "./protocol.ts";

export type * from "./protocol.ts";

export type Results = TracefsProtocol["results"];
export type Op = keyof Results;
export type ArgsOf<O extends Op> = Extract<Request, { op: O }> extends { args: infer A } ? A : never;

export const DEFAULT_SOCKET = "/run/tracefs/tracefs-daemon.sock";

export class TracefsError extends Error {
  code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "TracefsError";
    this.code = code;
  }
}

export class Client {
  socket: string;

  constructor(socket: string = process.env.TRACEFS_SOCKET ?? DEFAULT_SOCKET) {
    this.socket = socket;
  }

  call<O extends Op>(op: O, ...args: [ArgsOf<O>] extends [never] ? [] : [ArgsOf<O>]): Promise<Results[O]> {
    const req = args.length ? { op, args: args[0] } : { op };
    return new Promise((resolve, reject) => {
      const chunks: Buffer[] = [];
      const s = connect(this.socket, () => s.end(JSON.stringify(req) + "\n"));
      s.on("data", (c: Buffer) => chunks.push(c));
      s.on("error", reject);
      s.on("end", () => {
        let r: Response;
        try {
          r = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        } catch (e) {
          reject(new TracefsError("invalid", `bad response: ${e}`));
          return;
        }
        if (r.ok) resolve(r.result as Results[O]);
        else reject(new TracefsError(r.error?.code ?? "internal", r.error?.message ?? "unknown error"));
      });
    });
  }
}
