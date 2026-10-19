// simframe: a fake frame for development. Same engine as the real frame (WPE WebKit
// via cog --platform=headless), same agent (the real tfctl binary), no hardware.
//
// It runs three things:
//  1. WPEWebDriver, which launches cog in automation mode and gives us screenshots
//  2. tfctl agent, pointed at SERVER_URL with FRAME_ID/FRAME_TOKEN (writes /tmp/thinframe.conf)
//  3. a loop that follows FRAME_URL/COG_ROTATION from that conf (tfctl persists what the
//     server pushes), navigates, and writes /out/frame.png every SHOT_INTERVAL_S seconds,
//     rotated the way the panel would be.
//
// Env: SERVER_URL, FRAME_ID (default sim), FRAME_TOKEN, SHOT_INTERVAL_S (default 5), OUT_DIR (/out).
package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	"image/draw"
	"image/png"
	"io"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

const (
	driverURL = "http://127.0.0.1:4444"
	confPath  = "/tmp/thinframe.conf"
)

func env(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

func main() {
	log.SetFlags(log.Ltime)
	server := strings.TrimRight(env("SERVER_URL", "http://frameserver:8080"), "/")
	id := env("FRAME_ID", "sim")
	token := os.Getenv("FRAME_TOKEN")
	outDir := env("OUT_DIR", "/out")
	interval, _ := strconv.Atoi(env("SHOT_INTERVAL_S", "5"))
	if interval < 1 {
		interval = 1
	}
	_ = os.MkdirAll(outDir, 0o755)

	conf := fmt.Sprintf("SERVER_URL=%q\nFRAME_ID=%q\nFRAME_TOKEN=%q\nCOG_ROTATION=\"3\"\n", server, id, token)
	if err := os.WriteFile(confPath, []byte(conf), 0o600); err != nil {
		log.Fatal(err)
	}

	start("WPEWebDriver", "--host=127.0.0.1", "--port=4444")
	agent := exec.Command("tfctl", "agent")
	agent.Env = append(os.Environ(), "TFCTL_CONF="+confPath)
	agent.Stdout, agent.Stderr = os.Stdout, os.Stderr
	if err := agent.Start(); err != nil {
		log.Fatalf("tfctl agent: %v", err)
	}
	go func() { log.Printf("tfctl agent exited: %v", agent.Wait()); os.Exit(1) }()

	waitFor(driverURL + "/status")
	var sess, curURL string
	var pw, ph int
	for {
		url, rot := readConf(server, id)
		// cog's headless surface is fixed at 800x600; the page renders at the panel
		// size and scales to fit (server ?sim=WxH), and we crop the screenshot back.
		pw, ph = 1280, 800
		if rot%2 == 0 {
			pw, ph = 800, 1280
		}
		if strings.HasPrefix(url, server) {
			sep := "?"
			if strings.Contains(url, "?") {
				sep = "&"
			}
			url = fmt.Sprintf("%s%ssim=%dx%d", url, sep, pw, ph)
		}
		if sess == "" {
			sess = newSession()
			curURL = ""
			log.Printf("webdriver session %s", sess)
		}
		if url != curURL {
			wd("POST", "/session/"+sess+"/url", map[string]any{"url": url})
			log.Printf("navigate %s (panel %dx%d)", url, pw, ph)
			curURL = url
			time.Sleep(1500 * time.Millisecond)
		}
		if err := shoot(sess, filepath.Join(outDir, "frame.png"), pw, ph); err != nil {
			log.Printf("screenshot: %v", err)
			if strings.Contains(err.Error(), "invalid session") {
				sess = "" // recreate on the next loop
			}
		}
		time.Sleep(time.Duration(interval) * time.Second)
	}
}

func start(name string, args ...string) {
	c := exec.Command(name, args...)
	c.Stdout, c.Stderr = os.Stdout, os.Stderr
	if err := c.Start(); err != nil {
		log.Fatalf("%s: %v", name, err)
	}
	go func() { log.Printf("%s exited: %v", name, c.Wait()); os.Exit(1) }()
}

func waitFor(url string) {
	for i := 0; i < 100; i++ {
		if r, err := http.Get(url); err == nil {
			r.Body.Close()
			return
		}
		time.Sleep(200 * time.Millisecond)
	}
	log.Fatalf("%s never came up", url)
}

func wd(method, path string, body any) map[string]any {
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, _ := http.NewRequest(method, driverURL+path, rd)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		log.Printf("webdriver %s %s: %v", method, path, err)
		return nil
	}
	defer resp.Body.Close()
	var out struct {
		Value map[string]any `json:"value"`
	}
	raw, _ := io.ReadAll(resp.Body)
	if json.Unmarshal(raw, &out) != nil {
		// screenshot returns a bare string value
		var s struct {
			Value string `json:"value"`
		}
		if json.Unmarshal(raw, &s) == nil {
			return map[string]any{"_string": s.Value}
		}
	}
	if resp.StatusCode >= 400 {
		log.Printf("webdriver %s %s: %d %s", method, path, resp.StatusCode, raw)
		if out.Value == nil {
			out.Value = map[string]any{}
		}
		out.Value["_error"] = string(raw)
	}
	return out.Value
}

func newSession() string {
	caps := map[string]any{"capabilities": map[string]any{"alwaysMatch": map[string]any{
		"wpe:browserOptions": map[string]any{
			"binary": "/usr/bin/cog",
			"args":   []string{"--automation", "--platform=headless"},
		},
	}}}
	for i := 0; i < 10; i++ {
		v := wd("POST", "/session", caps)
		if sid, ok := v["sessionId"].(string); ok && sid != "" {
			return sid
		}
		time.Sleep(time.Second)
	}
	log.Fatal("could not create a WebDriver session")
	return ""
}

// readConf returns the page URL and rotation tfctl currently has persisted.
func readConf(server, id string) (string, int) {
	url, rot := server+"/frame/"+id, 3
	b, err := os.ReadFile(confPath)
	if err != nil {
		return url, rot
	}
	for _, l := range strings.Split(string(b), "\n") {
		k, v, ok := strings.Cut(strings.TrimSpace(l), "=")
		if !ok {
			continue
		}
		v = strings.Trim(strings.TrimSpace(v), `"`)
		switch strings.TrimSpace(k) {
		case "FRAME_URL":
			if v != "" {
				url = v
			}
		case "COG_ROTATION":
			if n, err := strconv.Atoi(v); err == nil && n >= 0 && n < 4 {
				rot = n
			}
		}
	}
	if strings.HasPrefix(url, "/") {
		url = server + url
	}
	return url, rot
}

func shoot(sess, path string, pw, ph int) error {
	v := wd("GET", "/session/"+sess+"/screenshot", nil)
	s, _ := v["_string"].(string)
	if s == "" {
		if e, _ := v["_error"].(string); strings.Contains(e, "invalid session") {
			return fmt.Errorf("invalid session")
		}
		return fmt.Errorf("empty screenshot")
	}
	raw, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return err
	}
	img, err := png.Decode(bytes.NewReader(raw))
	if err != nil {
		return err
	}
	// Crop to the scaled panel rectangle (the page scaled itself to fit the surface).
	b := img.Bounds()
	scale := min(float64(b.Dx())/float64(pw), float64(b.Dy())/float64(ph))
	cw, ch := int(float64(pw)*scale), int(float64(ph)*scale)
	rgba := image.NewRGBA(image.Rect(0, 0, cw, ch))
	draw.Draw(rgba, rgba.Bounds(), img, b.Min, draw.Src)
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if err := png.Encode(f, rgba); err != nil {
		f.Close()
		return err
	}
	f.Close()
	return os.Rename(tmp, path)
}
