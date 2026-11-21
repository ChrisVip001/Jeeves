
export interface UserPod {
  /** DNS-safe id, e.g. `u-3f9a…`. */
  id: string;
  image: string;
  /** Bearer token of the Pod's own dashboard API (never the model key). */
  opsToken: string;
  /** Base URL and per-user token of the metered model proxy. */
  modelBaseUrl: string;
  modelToken: string;
  awake: boolean;
  storage?: string;
  maxTurns?: number;
  homeMaxMb?: number;
}

type Obj = Record<string, unknown>;

export function userManifests(u: UserPod): Obj[] {
  const labels = { app: "jeeves", "jeeves/user": u.id };
  const meta = (name: string): Obj => ({ name, labels });
  const storage = u.storage ?? "5Gi";
  return [
    {
      apiVersion: "v1",
      kind: "Secret",
      metadata: meta(`${u.id}-env`),
      type: "Opaque",
      stringData: {
        JEEVES_OPS_TOKEN: u.opsToken,
        JEEVES_MODEL_TOKEN: u.modelToken,
      },
    },
    {
      apiVersion: "v1",
      kind: "PersistentVolumeClaim",
      metadata: meta(`${u.id}-home`),
      spec: {
        accessModes: ["ReadWriteOnce"],
        resources: { requests: { storage } },
      },
    },
    {
      apiVersion: "apps/v1",
      kind: "Deployment",
      metadata: meta(u.id),
      spec: {
        replicas: u.awake ? 1 : 0,
        strategy: { type: "Recreate" },
        selector: { matchLabels: labels },
        template: {
          metadata: { labels },
          spec: {
            automountServiceAccountToken: false,
            terminationGracePeriodSeconds: 30,
            securityContext: { runAsNonRoot: true, fsGroup: 1000 },
            initContainers: [
              {
                name: "seed-models",
                image: u.image,
                command: ["sh", "-c", `test -f /home/jeeves/.jeeves/models.json || printf '%s' '${seedModels(u.modelBaseUrl)}' > /home/jeeves/.jeeves/models.json`],
                volumeMounts: [{ name: "home", mountPath: "/home/jeeves/.jeeves" }],
              },
            ],
            containers: [
              {
                name: "jeeves",
                image: u.image,
                args: ["dashboard"],
                ports: [{ containerPort: 7777 }],
                env: [
                  { name: "JEEVES_HOME", value: "/home/jeeves/.jeeves" },
                  { name: "JEEVES_OPS_HOST", value: "0.0.0.0" },
                  { name: "JEEVES_MAX_TURNS", value: String(u.maxTurns ?? 2) },
                  { name: "JEEVES_HOME_MAX_MB", value: String(u.homeMaxMb ?? 4096) },
                  secretEnv("JEEVES_OPS_TOKEN", `${u.id}-env`),
                  secretEnv("JEEVES_MODEL_TOKEN", `${u.id}-env`),
                ],
                readinessProbe: { httpGet: { path: "/healthz", port: 7777 } },
                livenessProbe: {
                  httpGet: { path: "/healthz", port: 7777 },
                  initialDelaySeconds: 20,
                },
                volumeMounts: [{ name: "home", mountPath: "/home/jeeves/.jeeves" }],
                resources: {
                  requests: { cpu: "100m", memory: "256Mi" },
                  limits: { cpu: "1", memory: "1Gi" },
                },
              },
            ],
            volumes: [
              { name: "home", persistentVolumeClaim: { claimName: `${u.id}-home` } },
            ],
          },
        },
      },
    },
    {
      apiVersion: "v1",
      kind: "Service",
      metadata: meta(u.id),
      spec: { selector: labels, ports: [{ port: 7777, targetPort: 7777 }] },
    },
  ];
}

function seedModels(baseUrl: string): string {
  return JSON.stringify({
    active: "included",
    profiles: [
      { id: "included", provider: "anthropic", base_url: baseUrl, key_source: "env:JEEVES_MODEL_TOKEN" },
    ],
  }).replace(/'/g, "");
}

function secretEnv(name: string, secret: string): Obj {
  return { name, valueFrom: { secretKeyRef: { name: secret, key: name } } };
}

/** Where the control plane reaches a user's Pod inside the cluster. */
export function podUrl(id: string, namespace = "jeeves"): string {
  return `http://${id}.${namespace}.svc:7777`;
}
