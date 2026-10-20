package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"
)

// Weather is a current-conditions reading.
type Weather struct {
	TempC float64
	Desc  string
}

// WeatherSource provides current weather.
type WeatherSource interface{ Current() (Weather, error) }

// OpenMeteoURL is the public forecast endpoint (free, no key).
const OpenMeteoURL = "https://api.open-meteo.com/v1/forecast"

// OpenMeteo is a keyless adapter, cached 10 minutes. On a failed fetch it returns
// the last good reading (zero value if none) together with the error.
type OpenMeteo struct {
	Lat, Lon float64
	BaseURL  string // empty = OpenMeteoURL; tests point it at an httptest server
	mu       sync.Mutex
	last     Weather
	at       time.Time
}

var wmo = map[int]string{0: "Clear", 1: "Mostly clear", 2: "Partly cloudy", 3: "Overcast", 45: "Fog", 48: "Rime fog",
	51: "Light drizzle", 53: "Drizzle", 55: "Heavy drizzle", 61: "Light rain", 63: "Rain", 65: "Heavy rain",
	71: "Light snow", 73: "Snow", 75: "Heavy snow", 80: "Showers", 81: "Showers", 82: "Heavy showers", 95: "Thunderstorm"}

// Current fetches or returns the cached reading.
func (o *OpenMeteo) Current() (Weather, error) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if !o.at.IsZero() && time.Since(o.at) < 10*time.Minute {
		return o.last, nil
	}
	base := o.BaseURL
	if base == "" {
		base = OpenMeteoURL
	}
	url := fmt.Sprintf("%s?latitude=%.4f&longitude=%.4f&current=temperature_2m,weather_code", base, o.Lat, o.Lon)
	cl := http.Client{Timeout: 8 * time.Second}
	resp, err := cl.Get(url)
	if err != nil {
		return o.last, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return o.last, fmt.Errorf("weather: %s", resp.Status)
	}
	var body struct {
		Current *struct {
			T    float64 `json:"temperature_2m"`
			Code int     `json:"weather_code"`
		} `json:"current"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&body); err != nil {
		return o.last, err
	}
	if body.Current == nil {
		return o.last, fmt.Errorf("weather: response has no current block")
	}
	d := wmo[body.Current.Code]
	if d == "" {
		d = fmt.Sprintf("code %d", body.Current.Code)
	}
	o.last, o.at = Weather{TempC: body.Current.T, Desc: d}, time.Now()
	return o.last, nil
}
