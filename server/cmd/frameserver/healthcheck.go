package main

import (
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/MikeO7/thinframe/server/internal/discover"
)

// healthcheckTimeout bounds the probe; the image HEALTHCHECK allows a little more.
const healthcheckTimeout = 3 * time.Second

// healthcheck is `frameserver healthcheck`, the image HEALTHCHECK: it asks the server running in
// the same container for /healthz on loopback, so a hung or crashed HTTP server reads unhealthy.
// The image is scratch, so this replaces curl.
func healthcheck(port string, out io.Writer) int {
	if err := probe("http://127.0.0.1:"+port+"/healthz", healthcheckTimeout); err != nil {
		fmt.Fprintf(out, "unhealthy: %v\n", err)
		return 1
	}
	fmt.Fprintln(out, "healthy")
	return 0
}

func probe(url string, timeout time.Duration) error {
	c := &http.Client{Timeout: timeout}
	resp, err := c.Get(url)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 64))
	if err != nil {
		return fmt.Errorf("read %s: %w", url, err)
	}
	if resp.StatusCode != http.StatusOK || strings.TrimSpace(string(body)) != "ok" {
		return fmt.Errorf("%s answered %d %q", url, resp.StatusCode, body)
	}
	return nil
}

// listenPort is the HTTP port: PORT, else the discovery port.
func listenPort() string {
	return env("PORT", strconv.Itoa(discover.Port))
}
