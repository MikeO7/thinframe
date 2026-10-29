package api

import (
	"context"
	"errors"
	"fmt"
	"net/url"
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

func TestRedactErr(t *testing.T) {
	ue := &url.Error{Op: "Post", URL: "https://user:pw@ha.local:8123/api/webhook/SECRET?key=SECRET", Err: errors.New("dial tcp: refused")}
	for _, err := range []error{ue, fmt.Errorf("feed 2: %w", ue), errors.Join(errors.New("x"), ue)} {
		got := redactErr(err)
		if strings.Contains(got, "SECRET") || strings.Contains(got, "pw") || !strings.Contains(got, "https://ha.local:8123/...") || !strings.Contains(got, "refused") {
			t.Errorf("redactErr(%v) = %q", err, got)
		}
	}
	if redactErr(nil) != "" || redactErr(errors.New("weather: 503")) != "weather: 503" {
		t.Error("plain errors")
	}
}
