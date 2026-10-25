package api

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os/exec"
	"sort"
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
		cs = append(cs, container(f[0], f[1], f[2]))
	}
	return cs
}

// container folds docker's health check result, which is only in the status text ("Up 3 days
// (healthy)", "(unhealthy)", "(health: starting)"), into the state.
func container(name, state, status string) Container {
	c := Container{Name: name, State: state}
	switch {
	case strings.Contains(status, "(unhealthy)"):
		c.State = "unhealthy"
	case strings.Contains(status, "(health: starting)"):
		c.State = "starting"
	case strings.Contains(status, "(healthy)"):
		c.State = "healthy"
	}
	if c.State == "exited" || c.State == "restarting" || c.State == "dead" {
		c.Note = status
	}
	return c
}

// ParseDockerAPI reads GET /containers/json?all=1 from the Docker Engine API.
func ParseDockerAPI(r io.Reader) ([]Container, error) {
	var raw []struct {
		Names  []string
		State  string
		Status string
	}
	if err := json.NewDecoder(io.LimitReader(r, 4<<20)).Decode(&raw); err != nil {
		return nil, err
	}
	cs := make([]Container, 0, len(raw))
	for _, x := range raw {
		if len(x.Names) == 0 {
			continue
		}
		cs = append(cs, container(strings.TrimPrefix(x.Names[0], "/"), x.State, x.Status))
	}
	sort.Slice(cs, func(i, j int) bool { return cs[i].Name < cs[j].Name })
	return cs, nil
}

// PollDocker reads the container list from a Docker socket (in a container: the host's
// /var/run/docker.sock mounted in) every interval into the source "services". It only ever
// sends GET /containers/json.
func (s *Server) PollDocker(ctx context.Context, sock string, every time.Duration) {
	cl := &http.Client{Timeout: 15 * time.Second, Transport: &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return (&net.Dialer{}).DialContext(ctx, "unix", sock)
		}}}
	for {
		if err := s.pollDocker(ctx, cl); err != nil {
			log.Printf("services: docker %s: %v", sock, err)
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(every):
		}
	}
}

func (s *Server) pollDocker(ctx context.Context, cl *http.Client) error {
	req, err := http.NewRequestWithContext(ctx, "GET", "http://docker/containers/json?all=1", nil)
	if err != nil {
		return err
	}
	resp, err := cl.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("%s", resp.Status)
	}
	cs, err := ParseDockerAPI(resp.Body)
	if err != nil {
		return err
	}
	b, err := json.Marshal(map[string]any{"containers": cs})
	if err != nil {
		return err
	}
	return s.SetSource("services", b)
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
	// No timestamp in the payload: an unchanged list is then an unchanged source, not re-sent.
	b, err := json.Marshal(map[string]any{"containers": ParseDockerPS(string(out))})
	if err != nil {
		return err
	}
	return s.SetSource("services", b)
}
