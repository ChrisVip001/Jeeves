package main

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"jeeves/internal/cloudkit"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	kubeschema "k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/dynamic"
)

type kubernetes struct {
	client                                 dynamic.Interface
	namespace, image, modelURL, gatewayURL string
	desktopKey                             []byte
	provisioning                           provisioning
}

func relayToken(key []byte, id string) string { return hex.EncodeToString(cloudkit.MAC(key, id)) }

type object = map[string]any

func (k kubernetes) url(id string) string {
	return fmt.Sprintf("http://%s.%s.svc:7777", id, k.namespace)
}
func (k kubernetes) ensure(ctx context.Context, t tenant) error {
	resources, err := k.objects(t)
	if err != nil {
		return err
	}
	force := true
	for _, o := range resources {
		data, err := json.Marshal(o)
		if err != nil {
			return err
		}
		gvk := o.obj.GroupVersionKind()
		gvr := kubeschema.GroupVersionResource{Group: gvk.Group, Version: gvk.Version, Resource: o.plural}
		if _, err := k.client.Resource(gvr).Namespace(k.namespace).Patch(ctx, o.obj.GetName(), types.ApplyPatchType, data,
			metav1.PatchOptions{FieldManager: "jeeves-control-plane", Force: &force}); err != nil {
			return fmt.Errorf("apply %s: %w", o.plural, err)
		}
	}
	return nil
}
func (k kubernetes) scale(ctx context.Context, id string, n int64) error {
	gvr := kubeschema.GroupVersionResource{Group: "apps", Version: "v1", Resource: "deployments"}
	_, err := k.client.Resource(gvr).Namespace(k.namespace).Patch(ctx, id, types.MergePatchType,
		[]byte(`{"spec":{"replicas":`+strconv.FormatInt(n, 10)+`}}`), metav1.PatchOptions{}, "scale")
	return err
}

type resource struct {
	obj    *unstructured.Unstructured
	plural string
}

func (r resource) MarshalJSON() ([]byte, error) { return json.Marshal(r.obj.Object) }
func (k kubernetes) objects(t tenant) ([]resource, error) {
	label := object{"app": "jeeves", "jeeves/user": t.id}
	env := t.id + "-env"
	volume := t.id + "-home"
	relay := t.id + "-relay"
	base := func(version, kind, plural, name string, fields object) resource {
		fields["apiVersion"], fields["kind"] = version, kind
		fields["metadata"] = object{"name": name, "namespace": k.namespace, "labels": label}
		return resource{&unstructured.Unstructured{Object: fields}, plural}
	}
	seed, err := modelsSeedJSON(k.provisioning)
	if err != nil {
		return nil, err
	}
	home := "/home/jeeves/.jeeves"
	mount := []object{{"name": "home", "mountPath": home}}
	secret := func(key string) object {
		return object{"name": key, "valueFrom": object{"secretKeyRef": object{"name": env, "key": key}}}
	}
	probe := object{"httpGet": object{"path": "/healthz", "port": 7777}}
	return []resource{
		base("v1", "Secret", "secrets", env, object{"type": "Opaque", "data": map[string][]byte{"JEEVES_OPS_TOKEN": []byte(t.ops), "JEEVES_MODEL_TOKEN": []byte(t.model)}}),
		base("v1", "Secret", "secrets", relay, object{"type": "Opaque", "data": map[string][]byte{
			"tenant": []byte(t.id), "token": []byte(relayToken(k.desktopKey, t.id)), "gateway": []byte(k.gatewayURL),
		}}),

		base("v1", "PersistentVolumeClaim", "persistentvolumeclaims", volume, object{"spec": object{
			"accessModes": []string{"ReadWriteOnce"}, "resources": object{"requests": object{"storage": "5Gi"}},
		}}),
		base("apps/v1", "Deployment", "deployments", t.id, object{"spec": object{
			"replicas": 1, "strategy": object{"type": "Recreate"}, "selector": object{"matchLabels": label},
			"template": object{"metadata": object{"labels": label}, "spec": object{
				"automountServiceAccountToken": false, "terminationGracePeriodSeconds": 30,
				"securityContext": object{"runAsNonRoot": true, "fsGroup": 1000},
				"initContainers": []object{{"name": "seed-models", "image": k.image,
					"command":      []string{"sh", "-c", `test -f "$1/models.json" || printf '%s' "$2" > "$1/models.json"`, "seed", home, string(seed)},
					"volumeMounts": mount}},
				"containers": []object{{"name": "jeeves", "image": k.image, "args": []string{"runtime"},
					"ports": []object{{"containerPort": 7777}},
					"env": []object{{"name": "JEEVES_HOME", "value": home}, {"name": "JEEVES_OPS_HOST", "value": "0.0.0.0"},
						{"name": "JEEVES_DESKTOP_HELPER", "value": "/usr/local/bin/jeeves-desktop-relay"}, {"name": "JEEVES_MAX_TURNS", "value": "2"}, {"name": "JEEVES_HOME_MAX_MB", "value": "4096"}, secret("JEEVES_OPS_TOKEN"), secret("JEEVES_MODEL_TOKEN")},
					"volumeMounts": append(mount, object{"name": "relay", "mountPath": "/var/run/jeeves-relay", "readOnly": true}), "readinessProbe": probe,
					"resources": object{"requests": object{"cpu": "100m", "memory": "256Mi"}, "limits": object{"cpu": "1", "memory": "1Gi"}},
				}},
				"volumes": []object{{"name": "home", "persistentVolumeClaim": object{"claimName": volume}}, {"name": "relay", "secret": object{"secretName": relay, "defaultMode": 288}}},
			}},
		}}),
		base("v1", "Service", "services", t.id, object{"spec": object{"selector": label, "ports": []object{{"port": 7777, "targetPort": 7777}}}}),
	}, nil
}
func (p *plane) healthy(ctx context.Context, id string) bool {
	req, err := http.NewRequestWithContext(ctx, "GET", p.cluster.url(id)+"/healthz", nil)
	if err != nil {
		return false
	}
	res, err := p.probe.Do(req)
	if err != nil {
		return false
	}
	defer res.Body.Close()
	return res.StatusCode == http.StatusOK
}
func (p *plane) reconcile(ctx context.Context, t tenant) {
	ready, after := false, 8*time.Second
	defer func() {
		if err := p.result(context.WithoutCancel(ctx), t, ready, after); err != nil {
			report(t.id, err)
		}
	}()
	if !t.wanted {
		if err := p.cluster.scale(ctx, t.id, 0); err != nil {
			ready = t.ready
			report(t.id, err)
		}
		return
	}
	ready = p.healthy(ctx, t.id)
	if !t.ready || !ready {
		if err := p.cluster.ensure(ctx, t); err != nil {
			report(t.id, err)
			ready = false
		}
		return
	}
	if time.Now().UnixMilli()-t.last < p.idle.Milliseconds() || !p.settled(ctx, t) {
		return
	}
	res, err := p.db.ExecContext(ctx, `UPDATE cp_tenants SET wanted=FALSE,ready=FALSE WHERE id=$1 AND last_active=$2 AND lease_until=$3`, t.id, t.last, t.lease)
	if err != nil {
		report(t.id, err)
		return
	}
	if n, _ := res.RowsAffected(); n != 1 {
		return
	}
	t.wanted, ready = false, false
	if err := p.cluster.scale(ctx, t.id, 0); err != nil {
		report(t.id, err)
		ready = true
	}
}
