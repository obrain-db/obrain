package obrain

import (
	"bytes"
	"os"
	"testing"
)

// TestHeaderIsFreshCopy guards against drift: go/obrain.h must be byte-identical
// to the cbindgen-generated crates/bindings/c/obrain.h. Regenerate by copying it.
func TestHeaderIsFreshCopy(t *testing.T) {
	generated, err := os.ReadFile("../c/obrain.h")
	if err != nil {
		t.Fatalf("reading generated header: %v", err)
	}
	local, err := os.ReadFile("obrain.h")
	if err != nil {
		t.Fatalf("reading local header: %v", err)
	}
	if !bytes.Equal(generated, local) {
		t.Fatal("go/obrain.h differs from ../c/obrain.h: copy the generated header into go/")
	}
}
