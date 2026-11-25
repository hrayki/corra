package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/modal-labs/libmodal/modal-go"
	pb "github.com/modal-labs/libmodal/modal-go/proto/modal_proto"
	"google.golang.org/grpc"
)

type request struct {
	Op        string            `json:"op"`
	App       string            `json:"app"`
	Registry  string            `json:"registry"`
	Commands  []string          `json:"commands"`
	ImageID   string            `json:"image_id"`
	SandboxID string            `json:"sandbox_id"`
	CPU       float64           `json:"cpu"`
	CPULimit  float64           `json:"cpu_limit"`
	MemoryMiB int               `json:"memory_mib"`
	MemLimit  int               `json:"memory_limit_mib"`
	Tags      map[string]string `json:"tags"`
	TimeoutS  int               `json:"timeout_s"`
	Env       map[string]string `json:"env"`
	Command   []string          `json:"command"`
	VM        bool              `json:"vm"`
}

func dnsReset(path string) string {
	return fmt.Sprintf("if [ -L %[1]s ] || [ ! -e %[1]s ]; then rm -f %[1]s && : > %[1]s; fi", path)
}

func resetDNS(ctx context.Context, sb *modal.Sandbox) error {
	p, err := sb.Exec(ctx, []string{"sh", "-c", dnsReset("/etc/resolv.conf")}, &modal.SandboxExecParams{
		Stdout:  modal.Ignore,
		Stderr:  modal.Ignore,
		Timeout: 30 * time.Second,
	})
	if err != nil {
		return err
	}
	code, err := p.Wait(ctx)
	if err != nil {
		return err
	}
	if code != 0 {
		return fmt.Errorf("exit code %d", code)
	}
	return nil
}

func fail(err error) {
	json.NewEncoder(os.Stdout).Encode(map[string]string{"error": err.Error()})
	os.Exit(1)
}

func main() {
	var req request
	if err := json.NewDecoder(os.Stdin).Decode(&req); err != nil {
		fail(fmt.Errorf("bad request: %w", err))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Minute)
	defer cancel()

	vmInterceptor := func(ctx context.Context, method string, r, reply any, cc *grpc.ClientConn, invoker grpc.UnaryInvoker, opts ...grpc.CallOption) error {
		if c, ok := r.(*pb.SandboxCreateRequest); ok && req.VM {
			if d := c.GetDefinition(); d != nil {
				d.SetRuntime("vm")
			}
		}
		return invoker(ctx, method, r, reply, cc, opts...)
	}
	client, err := modal.NewClientWithOptions(&modal.ClientParams{
		TokenID:               os.Getenv("MODAL_TOKEN_ID"),
		TokenSecret:           os.Getenv("MODAL_TOKEN_SECRET"),
		GRPCUnaryInterceptors: []grpc.UnaryClientInterceptor{vmInterceptor},
	})
	if err != nil {
		fail(err)
	}
	defer client.Close()

	appName := req.App
	if appName == "" {
		appName = "corra"
	}
	out := map[string]any{}
	switch req.Op {
	case "build_image":
		app, err := client.Apps.FromName(ctx, appName, &modal.AppFromNameParams{CreateIfMissing: true})
		if err != nil {
			fail(err)
		}
		img := client.Images.FromRegistry(req.Registry, nil)
		if len(req.Commands) > 0 {
			img = img.DockerfileCommands(req.Commands, nil)
		}
		built, err := img.Build(ctx, app)
		if err != nil {
			fail(err)
		}
		out["image_id"] = built.ImageID
	case "create":
		app, err := client.Apps.FromName(ctx, appName, &modal.AppFromNameParams{CreateIfMissing: true})
		if err != nil {
			fail(err)
		}
		img, err := client.Images.FromID(ctx, req.ImageID)
		if err != nil {
			fail(err)
		}
		timeout := time.Duration(req.TimeoutS) * time.Second
		if timeout == 0 {
			timeout = 24 * time.Hour
		}
		sb, err := client.Sandboxes.Create(ctx, app, img, &modal.SandboxCreateParams{
			CPU:            req.CPU,
			CPULimit:       req.CPULimit,
			MemoryMiB:      req.MemoryMiB,
			MemoryLimitMiB: req.MemLimit,
			Timeout:        timeout,
			Env:            req.Env,
			Command:        req.Command,
		})
		if err != nil {
			fail(err)
		}
		if len(req.Tags) > 0 {
			if err := sb.SetTags(ctx, req.Tags); err != nil {
				sb.Terminate(ctx, nil)
				fail(err)
			}
		}
		out["sandbox_id"] = sb.SandboxID
	case "list":
		app, err := client.Apps.FromName(ctx, appName, &modal.AppFromNameParams{CreateIfMissing: true})
		if err != nil {
			fail(err)
		}
		it, err := client.Sandboxes.List(ctx, &modal.SandboxListParams{AppID: app.AppID, Tags: req.Tags})
		if err != nil {
			fail(err)
		}
		ids := []string{}
		for sb, err := range it {
			if err != nil {
				fail(err)
			}
			ids = append(ids, sb.SandboxID)
		}
		out["sandbox_ids"] = ids
	case "delete_image":
		if err := client.Images.Delete(ctx, req.ImageID, nil); err != nil && !strings.Contains(err.Error(), "not found") {
			fail(err)
		}
		out["ok"] = true
	case "status":
		sb, err := client.Sandboxes.FromID(ctx, req.SandboxID)
		if err != nil {
			if strings.Contains(err.Error(), "not found") {
				out["status"] = "gone"
				break
			}
			fail(err)
		}
		code, err := sb.Poll(ctx)
		if err != nil {
			fail(err)
		}
		if code == nil {
			out["status"] = "running"
		} else {
			out["status"] = "gone"
			out["exit_code"] = *code
		}
	case "terminate":
		sb, err := client.Sandboxes.FromID(ctx, req.SandboxID)
		if err != nil {
			if strings.Contains(err.Error(), "not found") {
				out["ok"] = true
				break
			}
			fail(err)
		}
		if _, err := sb.Terminate(ctx, nil); err != nil && !strings.Contains(err.Error(), "not found") {
			fail(err)
		}
		out["ok"] = true
	case "snapshot":
		sb, err := client.Sandboxes.FromID(ctx, req.SandboxID)
		if err != nil {
			fail(err)
		}
		if err := resetDNS(ctx, sb); err != nil {
			fmt.Fprintf(os.Stderr, "couldn't check /etc/resolv.conf before the snapshot: %v\n", err)
		}
		img, err := sb.SnapshotFilesystem(ctx, 10*time.Minute)
		if err != nil {
			fail(err)
		}
		out["image_id"] = img.ImageID
	default:
		fail(fmt.Errorf("unknown op %q", req.Op))
	}
	json.NewEncoder(os.Stdout).Encode(out)
}
