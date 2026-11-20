
import { spawn } from "node:child_process";
import { userManifests, type UserPod } from "./manifests.ts";

export interface Cluster {
  apply(pod: UserPod): Promise<void>;
  scale(id: string, replicas: 0 | 1): Promise<void>;
  remove(id: string): Promise<void>;
}

function kubectl(args: string[], input?: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn("kubectl", args, { stdio: ["pipe", "ignore", "pipe"] });
    let err = "";
    child.stderr.on("data", (d) => (err += d));
    child.on("error", reject);
    child.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`kubectl ${args[0]}: ${err.trim()}`)),
    );
    child.stdin.end(input ?? "");
  });
}

export class Kubectl implements Cluster {
  namespace: string;
  constructor(namespace = "jeeves") {
    this.namespace = namespace;
  }
  async apply(pod: UserPod) {
    const list = { apiVersion: "v1", kind: "List", items: userManifests(pod) };
    await kubectl(["-n", this.namespace, "apply", "-f", "-"], JSON.stringify(list));
  }
  async scale(id: string, replicas: 0 | 1) {
    await kubectl(["-n", this.namespace, "scale", `deployment/${id}`, `--replicas=${replicas}`]);
  }
  async remove(id: string) {
    await kubectl(["-n", this.namespace, "delete", `deployment/${id}`, `service/${id}`, `secret/${id}-env`, "--ignore-not-found"]);
    await kubectl(["-n", this.namespace, "delete", "pvc", `${id}-home`, "--ignore-not-found"]);
  }
}

export class MemoryCluster implements Cluster {
  pods = new Map<string, { pod: UserPod; replicas: number }>();
  async apply(pod: UserPod) {
    this.pods.set(pod.id, { pod, replicas: pod.awake ? 1 : 0 });
  }
  async scale(id: string, replicas: 0 | 1) {
    const entry = this.pods.get(id);
    if (!entry) throw new Error(`no pod ${id}`);
    entry.replicas = replicas;
  }
  async remove(id: string) {
    this.pods.delete(id);
  }
}
