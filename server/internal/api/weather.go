package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"
)

// Weather is a current-conditions reading plus the daily forecast (°C; WMO weather codes).
type Weather struct {
	TempC float64
	Desc  string
	Code  int
	Night bool         // the sun is down now (open-meteo is_day = 0)
	Days  []WeatherDay // today first; empty when the feed sent no daily block
}

// WeatherDay is one day of the forecast. Date is YYYY-MM-DD in the place's own time zone.
type WeatherDay struct {
	Date       string
	Code       int
	MaxC, MinC float64
}

// WeatherSource provides current weather.
type WeatherSource interface{ Current() (Weather, error) }

// OpenMeteoURL is the public forecast endpoint (free, no key).
const OpenMeteoURL = "https://api.open-meteo.com/v1/forecast"

// OpenMeteo is a keyless adapter, cached 10 minutes. On a failed fetch it returns
// the last good reading (zero value if none) together with the error, and does not
// try again until the retry backoff (1 to 10 minutes) has passed.
type OpenMeteo struct {
	Lat, Lon float64
	BaseURL  string // empty = OpenMeteoURL; tests point it at an httptest server
	feed     refresher[Weather]
}

// refresher is the fetch policy of the weather and calendar feeds. A good result is
// kept for ttl. A failure keeps the last good value and blocks refetching for a backoff
// that starts at 1 minute and doubles up to 10, so a dead upstream is not hammered by
// every page request. At most one fetch runs at a time, outside the lock: other callers
// get the cached value at once, or wait for the fetch if nothing was ever fetched.
type refresher[T any] struct {
	mu       sync.Mutex
	val      T
	have     bool
	err      error     // error of the last fetch; nil after a success
	next     time.Time // no fetch before this
	backoff  time.Duration
	fetching bool
	done     chan struct{}    // closed when the running fetch ends
	clock    func() time.Time // nil = time.Now; tests move it
}

func (r *refresher[T]) now() time.Time {
	if r.clock != nil {
		return r.clock()
	}
	return time.Now()
}

func (r *refresher[T]) get(ttl time.Duration, fetch func() (T, error)) (T, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	switch {
	case r.fetching && !r.have:
		done := r.done
		r.mu.Unlock()
		<-done
		r.mu.Lock()
	case !r.fetching && !r.now().Before(r.next):
		r.fetching, r.done = true, make(chan struct{})
		r.mu.Unlock()
		v, err := fetch()
		r.mu.Lock()
		if err == nil {
			r.val, r.have, r.err, r.backoff = v, true, nil, 0
			r.next = r.now().Add(ttl)
		} else {
			r.backoff = min(max(2*r.backoff, time.Minute), 10*time.Minute)
			r.err, r.next = err, r.now().Add(r.backoff)
		}
		r.fetching = false
		close(r.done)
	}
	return r.val, r.err
}

var wmo = map[int]string{0: "Clear", 1: "Mostly clear", 2: "Partly cloudy", 3: "Overcast", 45: "Fog", 48: "Rime fog",
	51: "Light drizzle", 53: "Drizzle", 55: "Heavy drizzle", 61: "Light rain", 63: "Rain", 65: "Heavy rain",
	71: "Light snow", 73: "Snow", 75: "Heavy snow", 80: "Showers", 81: "Showers", 82: "Heavy showers", 95: "Thunderstorm"}

// Current fetches or returns the cached reading.
func (o *OpenMeteo) Current() (Weather, error) {
	return o.feed.get(10*time.Minute, o.fetch)
}

func (o *OpenMeteo) fetch() (Weather, error) {
	base := o.BaseURL
	if base == "" {
		base = OpenMeteoURL
	}
	url := fmt.Sprintf("%s?latitude=%.4f&longitude=%.4f&current=temperature_2m,weather_code,is_day"+
		"&daily=weather_code,temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=7", base, o.Lat, o.Lon)
	cl := http.Client{Timeout: 8 * time.Second}
	resp, err := cl.Get(url)
	if err != nil {
		return Weather{}, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return Weather{}, fmt.Errorf("weather: %s", resp.Status)
	}
	var body struct {
		Current *struct {
			T    float64 `json:"temperature_2m"`
			Code int     `json:"weather_code"`
			Day  *int    `json:"is_day"`
		} `json:"current"`
		Daily *struct {
			Time []string  `json:"time"`
			Code []int     `json:"weather_code"`
			Max  []float64 `json:"temperature_2m_max"`
			Min  []float64 `json:"temperature_2m_min"`
		} `json:"daily"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&body); err != nil {
		return Weather{}, err
	}
	if body.Current == nil {
		return Weather{}, fmt.Errorf("weather: response has no current block")
	}
	d := wmo[body.Current.Code]
	if d == "" {
		d = fmt.Sprintf("code %d", body.Current.Code)
	}
	wx := Weather{TempC: body.Current.T, Desc: d, Code: body.Current.Code, Night: body.Current.Day != nil && *body.Current.Day == 0}
	if dl := body.Daily; dl != nil {
		for i := range dl.Time {
			if i >= len(dl.Code) || i >= len(dl.Max) || i >= len(dl.Min) || i >= 7 {
				break
			}
			wx.Days = append(wx.Days, WeatherDay{Date: dl.Time[i], Code: dl.Code[i], MaxC: dl.Max[i], MinC: dl.Min[i]})
		}
	}
	return wx, nil
}
