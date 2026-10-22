//! The look of the pages that wait for a person (Wi-Fi setup, PAIR) under `SPLASH_STYLE=dawn`:
//! no cards, the content sits straight on the frosted sheet of the page backdrop (dawn.rs with an
//! empty title), thin Poppins headings, ink buttons. Class names are those of wifisetup.rs and
//! pair.rs, so one markup serves both looks; the seventies CSS stays in those files.
//! Sizes are in vh/vw on the 1280x800 page after cog's rotation, like the other pages.

pub const INK: &str = "#2b2140";

/// The shared stylesheet. `regular` and `light` are the font URLs as the page can reach them
/// (`file:///...` from the pairing page, `/poppins.ttf` from the Wi-Fi server); `bg` is the
/// backdrop's `url(..) center/cover no-repeat`, or empty.
pub fn css(regular: &str, light: &str, bg: &str) -> String {
    format!(
        "@font-face{{font-family:Poppins;font-weight:400;src:url(\"{regular}\")}}\
@font-face{{font-family:Poppins;font-weight:200;src:url(\"{light}\")}}\
html,body{{margin:0;height:100%;overflow:hidden;background:#f6c8c0 {bg};\
user-select:none;-webkit-user-select:none;touch-action:manipulation}}\
body{{color:{INK};font:3.2vh/1.35 Poppins,'DejaVu Sans',sans-serif}}\
.card{{position:absolute;left:8vw;top:12vh;width:84vw;max-height:76vh;box-sizing:border-box}}\
.wide{{top:11vh;max-height:none}}.low{{top:30vh;width:60vw}}\
h1{{margin:0 0 .15em;font-size:8vh;font-weight:200;line-height:1.05;letter-spacing:-.02em}}\
p{{margin:.2em 0 .7em;color:#4a3d63}}.note{{font-size:2.6vh}}\
.steps{{position:absolute;right:8vw;top:9vh;font-size:2.6vh;color:#4a3d63;\
background:rgba(255,255,255,.55);border:1px solid rgba(255,255,255,.9);border-radius:999px;padding:.35em 1em}}\
.steps b{{color:{INK};font-weight:400}}\
.err{{color:#8a1f4b;background:rgba(255,255,255,.6);border:1px solid rgba(138,31,75,.35);\
border-radius:1.6vh;padding:.4em .8em;font-size:2.8vh}}\
ul{{list-style:none;margin:0 0 2vh;padding:0;max-height:50vh;overflow-y:auto}}\
li a{{display:flex;align-items:center;gap:1.2vw;color:inherit;text-decoration:none;padding:1.7vh 1.2vw;\
border-bottom:1px solid rgba(43,33,64,.16)}}li:last-child a{{border:0}}\
li a:active{{background:rgba(255,255,255,.55);border-radius:1.6vh}}\
.name{{flex:1;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}}.dim{{opacity:.45}}\
.sig{{display:flex;align-items:flex-end;gap:.25vw;height:3.4vh}}.sig i{{width:.6vw;background:{INK};opacity:.22}}\
.sig i.on{{opacity:1}}.sig i:nth-child(1){{height:25%}}.sig i:nth-child(2){{height:50%}}.sig i:nth-child(3){{height:75%}}\
.sig i:nth-child(4){{height:100%}}.lock{{opacity:.7}}.lock svg{{height:3vh;display:block}}\
button,.btn{{font:3.6vh Poppins,'DejaVu Sans',sans-serif;color:#fff;background:{INK};border:0;border-radius:999px;\
padding:.45em 1.5em;text-decoration:none;display:inline-block;box-shadow:0 1vh 2.4vh rgba(43,33,64,.28)}}\
button:active,.btn:active{{transform:scale(.97)}}button:disabled{{opacity:.35}}\
.low button{{font-size:6vh;padding:.3em 1.8em}}\
.ghost{{background:rgba(255,255,255,.6);color:{INK};border:1px solid rgba(255,255,255,.95);\
box-shadow:0 .8vh 2vh rgba(80,50,120,.15)}}\
small{{position:absolute;left:8vw;bottom:9vh;font-size:2.4vh;color:#4a3d63}}\
.dots span{{animation:b 1.2s infinite}}.dots span:nth-child(2){{animation-delay:.2s}}.dots span:nth-child(3){{animation-delay:.4s}}\
@keyframes b{{0%,60%,100%{{opacity:.2}}30%{{opacity:1}}}}"
    )
}

/// The password page's own rules: field, rows and the on-screen keyboard.
pub const KEYBOARD_CSS: &str = ".wide h1{font-size:5.6vh}.top{display:flex;align-items:center;gap:2vw}\
.field{flex:1;font:4.2vh 'DejaVu Sans Mono',monospace;background:rgba(255,255,255,.7);\
border:1px solid rgba(43,33,64,.28);border-radius:1.6vh;padding:.3em .6em;white-space:pre;overflow:hidden}\
.row{display:flex;gap:1.5vw;align-items:center;margin:0}\
#kb{margin:1.6vh 0 0}#kb div{display:flex;justify-content:center;gap:.6vw;margin-bottom:1.2vh}\
#kb button{font:4.2vh 'DejaVu Sans Mono',monospace;min-width:7.6vw;height:9.4vh;padding:0;border-radius:1.8vh;\
background:rgba(255,255,255,.66);color:#2b2140;border:1px solid rgba(255,255,255,.95);\
box-shadow:0 .5vh 1.4vh rgba(80,50,120,.18)}\
#kb button.k2{background:rgba(255,255,255,.38);min-width:10.5vw;font:3.2vh Poppins,sans-serif}\
#kb button.on{background:#2b2140;color:#fff}#kb button.sp{min-width:44vw;font:3.2vh Poppins,sans-serif}\
small,.steps{display:none}.hint{opacity:.4}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylesheet_names_the_fonts_and_the_backdrop_and_balances_braces() {
        let c = css(
            "/poppins.ttf",
            "/poppins-light.ttf",
            "url(/backdrop.png) center/cover no-repeat",
        );
        assert!(c.contains("url(\"/poppins.ttf\")") && c.contains("url(\"/poppins-light.ttf\")"));
        assert!(c.contains("url(/backdrop.png)"));
        assert_eq!(c.matches('{').count(), c.matches('}').count());
        assert_eq!(
            KEYBOARD_CSS.matches('{').count(),
            KEYBOARD_CSS.matches('}').count()
        );
    }
}
