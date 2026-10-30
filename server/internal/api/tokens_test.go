package api

import "testing"

func TestParseTokens(t *testing.T) {
	long1, long2 := "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
	got, refused := ParseTokens(" " + long1 + ",short\n" + long2 + " ,, ")
	if len(got) != 2 || got[0] != long1 || got[1] != long2 || refused != 1 {
		t.Fatalf("ParseTokens = %q, %d", got, refused)
	}
	if got, refused := ParseTokens(""); got != nil || refused != 0 {
		t.Fatalf("empty = %q, %d", got, refused)
	}
}
