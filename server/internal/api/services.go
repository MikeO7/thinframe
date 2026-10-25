package api

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os/exec"
	"strings"
	"time"
)

// Container is one line of `docker ps -a` as the services widget reads it.
type Container struct {
	Name  string `json:"name"`
	State string `json:"state"` // healthy, running, starting, unhealthy, exited, other docker states
	Note  string `json:"note,omitempty"`
}

// psFormat is what PollServices asks docker for: name, state, status, tab-separated.
const psFormat = "{{.Names}}\t{{.State}}\t{{.Status}}"

// ParseDockerPS reads psFormat lines. The health check result, when there is one, is in
// the status text: "Up 3 days (healthy)", "(unhealthy)", "(health: starting)".
func ParseDockerPS(out string) []Container {
	var cs []Container
	sc := bufio.NewScanner(strings.NewReader(out))
	for sc.Scan() {
		f := strings.Split(sc.Text(), "\t")
		if len(f) < 3 || f[0] == "" {
			continue
		}
		c := Container{Name: f[0], State: f[1]}
		switch st := f[2]; {
		case strings.Contains(st, "(unhealthy)"):
			c.State = "unhealthy"
		case strings.Contains(st, "(health: starting)"):
			c.State = "starting"
		case strings.Contains(st, "(healthy)"):
			c.State = "healthy"
		}
		if c.State == "exited" || c.State == "restarting" {
			c.Note = f[2]
		}
		cs = append(cs, c)
	}
	return cs
}

// PollServices runs `docker ps` on host over ssh every interval and stores the result as the
// source "services". host is an ssh destination (an alias from ~/.ssh/config); it needs
// key auth, since BatchMode never prompts.
func (s *Server) PollServices(ctx context.Context, host string, every time.Duration) {
	if strings.HasPrefix(host, "-") {
		log.Printf("services: refusing ssh destination %q", host)
		return
	}
	for {
		if err := s.pollServices(ctx, host); err != nil {
			log.Printf("services: %v", err)
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(every):
		}
	}
}

func (s *Server) pollServices(ctx context.Context, host string) error {
	c, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	out, err := exec.CommandContext(c, "ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--", host,
		"docker", "ps", "-a", "--format", "'"+psFormat+"'").Output()
	if err != nil {
		return fmt.Errorf("ssh %s: %v", host, err)
	}
	b, err := json.Marshal(map[string]any{"containers": ParseDockerPS(string(out)), "updated": time.Now().UTC().Format(time.RFC3339)})
	if err != nil {
		return err
	}
	return s.SetSource("services", b)
}
