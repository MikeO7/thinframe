package api

import (
	"context"
	"strings"
	"testing"
)

func TestHookFailureDoesNotShowTheURL(t *testing.T) {
	s := &Server{}
	res := s.runHook(context.Background(), Hook{Method: "POST", URL: "http://127.0.0.1:1/api/webhook/secret-id-123", TimeoutS: 2}, hookVars{name: "car"})
	if res.OK || res.Error == "" || strings.Contains(res.Error, "secret-id-123") || strings.Contains(res.Error, "127.0.0.1") {
		t.Fatalf("error %q leaks the hook URL", res.Error)
	}
}
