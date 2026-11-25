package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func reset(t *testing.T, path string) {
	t.Helper()
	if out, err := exec.Command("sh", "-c", dnsReset(path)).CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
}

func TestADanglingResolvConfLinkBecomesAnEmptyFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "resolv.conf")
	if err := os.Symlink("../run/systemd/resolve/stub-resolv.conf", path); err != nil {
		t.Fatal(err)
	}
	reset(t, path)
	fi, err := os.Lstat(path)
	if err != nil {
		t.Fatal(err)
	}
	if !fi.Mode().IsRegular() || fi.Size() != 0 {
		t.Fatalf("want an empty plain file, got %v of %d bytes", fi.Mode(), fi.Size())
	}
}

func TestALinkToAWorkingFileIsReplacedToo(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "stub")
	os.WriteFile(target, []byte("nameserver 127.0.0.53\n"), 0o644)
	path := filepath.Join(dir, "resolv.conf")
	os.Symlink(target, path)
	reset(t, path)
	fi, _ := os.Lstat(path)
	if !fi.Mode().IsRegular() {
		t.Fatalf("want a plain file, got %v", fi.Mode())
	}
	if b, _ := os.ReadFile(target); string(b) != "nameserver 127.0.0.53\n" {
		t.Fatalf("the link's target changed: %q", b)
	}
}

func TestAPlainResolvConfIsLeftAlone(t *testing.T) {
	path := filepath.Join(t.TempDir(), "resolv.conf")
	os.WriteFile(path, []byte("nameserver 172.21.0.1\n"), 0o644)
	reset(t, path)
	if b, _ := os.ReadFile(path); string(b) != "nameserver 172.21.0.1\n" {
		t.Fatalf("a plain file was changed: %q", b)
	}
}
