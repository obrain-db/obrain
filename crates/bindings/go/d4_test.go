package obrain

import (
	"path/filepath"
	"testing"
)

// Decision D4: Open is read-only; OpenWrite takes the write lease (bail).

func TestOpenWriteCreatesThenOpenReadOnlyReads(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "store")

	w, err := OpenWrite(dir)
	if err != nil {
		t.Fatalf("OpenWrite: %v", err)
	}
	if _, err := w.Execute(`CREATE (:Person {name: 'Alix'})`); err != nil {
		t.Fatalf("write through OpenWrite: %v", err)
	}
	if err := w.Close(); err != nil {
		t.Fatalf("Close writer: %v", err)
	}

	r, err := Open(dir)
	if err != nil {
		t.Fatalf("Open (read-only) on existing store: %v", err)
	}
	defer r.Close()
	if r.NodeCount() != 1 {
		t.Errorf("read-only view: expected 1 node, got %d", r.NodeCount())
	}
}

func TestOpenReadOnlyRejectsMutation(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "store")
	w, err := OpenWrite(dir)
	if err != nil {
		t.Fatalf("OpenWrite: %v", err)
	}
	w.Close()

	r, err := Open(dir)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer r.Close()
	if _, err := r.Execute(`CREATE (:Person {name: 'Bob'})`); err == nil {
		t.Fatal("expected a mutation through read-only Open to fail")
	}
}

func TestOpenReadOnlyMissingStoreFails(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "absent")
	if _, err := Open(dir); err == nil {
		t.Fatal("expected Open on a non-existing store to fail (no store is created)")
	}
}

func TestOpenWriteLeaseIsExclusive(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "store")
	w, err := OpenWrite(dir)
	if err != nil {
		t.Fatalf("first OpenWrite: %v", err)
	}
	defer w.Close()

	if second, err := OpenWrite(dir); err == nil {
		second.Close()
		t.Fatal("expected a second OpenWrite on the same store to fail (write lease held)")
	}
}
