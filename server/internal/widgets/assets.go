package widgets

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"image"
	"image/color"
	"image/png"
	"math/rand/v2"
)

// The page's CSS and JS as static files named by their content (page-<hash>.css), so a browser
// keeps them for good and a new server build gets new names (docs/decisions.md "Page speed").
// Built once at start: one script, one stylesheet per theme, and Aurora's grain tile.

type asset struct {
	body  []byte
	ctype string
}

var (
	assets = map[string]asset{}
	jsName string
	// cssNames maps a theme ("" = the base) to its stylesheet's name.
	cssNames = map[string]string{}
)

// Asset returns a static file by its hashed name and its Content-Type.
func Asset(name string) ([]byte, string, bool) {
	a, ok := assets[name]
	return a.body, a.ctype, ok
}

// ScriptName is the page script's static name (page-<hash>.js).
func ScriptName() string { return jsName }

func cssFor(theme string) string {
	if n, ok := cssNames[theme]; ok {
		return n
	}
	return cssNames[""]
}

// addAsset stores b as base-<hash>.ext and returns that name.
func addAsset(base, ext, ctype string, b []byte) string {
	sum := sha256.Sum256(b)
	name := base + "-" + hex.EncodeToString(sum[:6]) + "." + ext
	assets[name] = asset{body: b, ctype: ctype}
	return name
}

func init() {
	read := func(n string) []byte {
		b, err := files.ReadFile("templates/" + n)
		if err != nil {
			panic(err)
		}
		return b
	}
	cat := func(names ...string) []byte {
		var out []byte
		for i, n := range names {
			if i > 0 {
				out = append(out, '\n')
			}
			out = append(out, read(n)...)
		}
		return out
	}
	jsName = addAsset("page", "js", "text/javascript; charset=utf-8", read("page.js"))
	grain := addAsset("grain", "png", "image/png", grainPNG())
	css := func(names ...string) string {
		b := bytes.ReplaceAll(cat(names...), []byte("/static/grain.png"), []byte("/static/"+grain))
		return addAsset("page", "css", "text/css; charset=utf-8", b)
	}
	cssNames[""] = css("page.css")
	// obsidian is a theme of its own and the base of aurora; Inter is obsidian's alone (aurora
	// asked for /api/data/asset/inter.woff2 and got a 404 on every load).
	cssNames["obsidian"] = css("page.css", "obsidian.css", "obsidian-font.css")
	cssNames["aurora"] = css("page.css", "obsidian.css", "aurora.css")
}

// grainPNG is Aurora's film grain, baked: a 180 px tile of white at about 2 % alpha with a
// little noise in it, the same strength as the SVG feTurbulence tile at 7 % opacity it
// replaces (alpha .55 x fractal noise around .5 x .07). As an image in the page's own
// background it needs no layer of its own and no filter run. Seeded, so its hash is stable.
func grainPNG() []byte {
	const size, levels = 180, 8
	pal := make(color.Palette, levels)
	for i := range pal {
		pal[i] = color.NRGBA{255, 255, 255, uint8(2 + i)} // alpha 2 to 9 of 255
	}
	img := image.NewPaletted(image.Rect(0, 0, size, size), pal)
	r := rand.New(rand.NewPCG(1, 2))
	for i := range img.Pix {
		// mean 4.9/255 (= 255 x .07 x .55 x .5), spread about 1.5/255
		v := 4.9 + 1.5*r.NormFloat64() - 2
		img.Pix[i] = uint8(min(max(int(v+0.5), 0), levels-1))
	}
	var b bytes.Buffer
	if err := (&png.Encoder{CompressionLevel: png.BestCompression}).Encode(&b, img); err != nil {
		panic(err)
	}
	return b.Bytes()
}
