# Renders the animated dither hero used at the top of the README.
# Requires: Python 3.10+, numpy, pillow, scipy.  Run: python assets/brand/src/dither_hero.py
import numpy as np, sys
from PIL import Image
from scipy import ndimage

import os
BR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..") + "/"
OUT = BR
GW, GH = 600, 200          # dither grid
PX = 2                     # output pixel size -> 1200x400
N = 200                    # frames, 8 s at 25 fps, seamless loop
TAU = 2*np.pi

# ---- bayer 8x8
def bayer(n):
    if n == 1: return np.array([[0]])
    b = bayer(n//2)
    return np.block([[4*b, 4*b+2],[4*b+3, 4*b+1]])
B8 = (bayer(8)+0.5)/64.0
TH = np.tile(B8, (GH//8+1, GW//8+1))[:GH, :GW]

# ---- tern mark mask (high res for rotation), grid height ~ 128
mark = Image.open(BR+"tern-mark.png").convert("L")
MH = 110
MW = int(mark.width*MH/mark.height)
PAD = 22                   # grid px of room around the bird for wing travel
S2 = 3                     # supersample for the wing warp
mk = np.array(mark.resize((MW*S2, MH*S2), Image.LANCZOS))/255.0
mk = np.pad(mk, PAD*S2)
HH, WW = mk.shape
gy, gx = np.mgrid[0:HH, 0:WW].astype(float)
# mark-space coords (0..1400, 0..1112) of each supersampled pixel
sc = mark.width/(MW*S2)
ux = (gx - PAD*S2)*sc; uy = (gy - PAD*S2)*sc
PIV = np.array([985.0, 468.0])          # shoulder
# wing weight: above the line (160,430)-(985,468), fading out toward the head
lx0, ly0, lx1, ly1 = 160.0, 430.0, 985.0, 468.0
nx, ny = (ly1-ly0), -(lx1-lx0); nn = np.hypot(nx, ny); nx/=nn; ny/=nn
if ny > 0: nx, ny = -nx, -ny             # normal points up
above = (ux-lx0)*nx + (uy-ly0)*ny
W_WING = np.clip(above/70.0, 0, 1)**1.2 * np.clip((1010-ux)/120.0, 0, 1)

def warped_mark(theta_deg, sy):
    th = np.radians(theta_deg)*W_WING
    s = 1 - (1-sy)*W_WING
    dx, dy = ux-PIV[0], uy-PIV[1]
    # inverse: rotate back, then un-squash along y
    c, sn = np.cos(-th), np.sin(-th)
    rx = c*dx - sn*dy; ry = sn*dx + c*dy
    ry = ry/np.maximum(s, 1e-3)
    srcx = (rx+PIV[0])/sc + PAD*S2; srcy = (ry+PIV[1])/sc + PAD*S2
    out = ndimage.map_coordinates(mk, [srcy, srcx], order=1, mode='constant')
    im = Image.fromarray((out*255).astype(np.uint8)).resize((MW+2*PAD, MH+2*PAD), Image.BOX)
    return np.array(im)/255.0

# ---- wordmark from lockup alpha (text part only)
lk = Image.open(BR+"tern-lockup-on-dark.png")
a = np.array(lk.split()[-1])
txt = Image.fromarray(a[:, 180:])
bbox = txt.getbbox(); txt = txt.crop(bbox)
TH_txt = 42
txt = txt.resize((int(txt.width*TH_txt/txt.height), TH_txt), Image.LANCZOS)
T = np.array(txt)/255.0

yy, xx = np.mgrid[0:GH, 0:GW].astype(float)

def pnoise(x, y, t, seed):
    # periodic-in-time smooth field built from sines
    rng = np.random.default_rng(seed)
    v = np.zeros_like(x)
    for k in range(6):
        fx, fy = rng.uniform(0.01, 0.05, 2)*rng.choice([-1,1],2)
        ph = rng.uniform(0, TAU)
        sp = rng.integers(1, 3)*rng.choice([-1,1])
        v += np.sin(fx*x + fy*y + ph + sp*TAU*t)
    return v/6

# drifting particles (speed lines / motes), periodic
rng = np.random.default_rng(7)
NP = 140
p0x = rng.uniform(0, GW, NP); p0y = rng.uniform(0, GH, NP)
pv = rng.integers(2, 6, NP)          # laps per loop -> seamless
pb = rng.uniform(0.35, 1.0, NP)
pl = rng.integers(2, 10, NP)          # streak length

def place(img, layer, mask, ox, oy):
    """Write layer into img where mask is set, clipped to the canvas."""
    h, w = mask.shape
    y0, x0 = max(oy, 0), max(ox, 0)
    y1, x1 = min(oy+h, GH), min(ox+w, GW)
    if y1 <= y0 or x1 <= x0: return
    m = mask[y0-oy:y1-oy, x0-ox:x1-ox]; l = layer[y0-oy:y1-oy, x0-ox:x1-ox]
    reg = img[y0:y1, x0:x1]; reg[m] = l[m]

def bird_at(t):
    """Position, depth, and a shaded bird sprite for loop time t."""
    th = TAU*t
    cx, cy, ax, ay = GW/2, GH/2, GW*0.40, GH*0.27
    px = cx + ax*np.sin(th); py = cy + ay*np.sin(2*th) + 26*np.cos(th)
    z = np.cos(th)                                   # +1 in front of the text, -1 behind
    vx = ax*np.cos(th); vy = 2*ay*np.cos(2*th) - 26*np.sin(th)
    heading = np.degrees(np.arctan2(vy, abs(vx)+1e-6))
    rot = float(np.clip(28 - 0.55*heading, -10, 38))      # the mark dives at about 28 degrees
    face = np.tanh(vx/(0.10*ax))                     # turns through edge-on at the ends
    scale = 0.74 + 0.24*z
    ph = TAU*16*t                                    # 16 wingbeats per loop
    u = np.sin(ph)
    m = warped_mark(-10 + 22*u, 0.80 + 0.20*(u+1)/2)
    im = Image.fromarray((m*255).astype(np.uint8))
    if face < 0: im = im.transpose(Image.FLIP_LEFT_RIGHT)
    w = max(4, int(im.width*scale*max(abs(face), 0.30))); h = max(4, int(im.height*scale))
    im = im.resize((w, h), Image.LANCZOS)
    im = im.rotate(rot if face >= 0 else -rot, resample=Image.BICUBIC, expand=True)
    m = np.array(im)/255.0
    inside = m > 0.5
    dist = np.clip(ndimage.distance_transform_edt(inside)/(5.0*scale+1e-3), 0, 1)
    my, mx = np.mgrid[0:m.shape[0], 0:m.shape[1]]
    sweep = np.exp(-(((mx+my*0.6) - (m.shape[0]+m.shape[1])*((t*3) % 1.0)*1.3 + 20)/14.0)**2)
    depth = 0.70 + 0.30*(z+1)/2                     # farther = dimmer, sparser dither
    shade = np.clip((0.74 + 0.22*(1-dist) + 0.28*sweep)*depth, 0, 1)
    py += 2.0*np.cos(ph)*scale                      # lift on each downstroke
    ox, oy = int(round(px - m.shape[1]/2)), int(round(py - m.shape[0]/2))
    return ox, oy, z, inside, shade, (px, py)

def frame(t, invert=False):
    # fog
    fog = pnoise(xx, yy, t, 3)*0.5 + pnoise(xx*1.7, yy*1.7, t, 11)*0.5
    fog = np.clip((fog-0.05)*0.42, 0, 0.22)
    vig = np.clip(1.15 - (((xx-GW/2)/(GW*0.62))**2 + ((yy-GH/2)/(GH*0.9))**2), 0, 1)
    img = fog*vig

    # drifting motes, parallax by brightness
    for i in range(NP):
        x = (p0x[i] - pv[i]*GW*t) % GW
        y = p0y[i] + 2*np.sin(TAU*(t+i*0.13))
        xi, yi = int(x), int(y)
        if 0 <= yi < GH:
            for k in range(pl[i]):
                xk = (xi+k) % GW
                img[yi, xk] = max(img[yi, xk], pb[i]*(1-k/pl[i])*0.8)

    # wake: faint dots along the last stretch of the flight path
    for k in range(1, 26):
        tk = t - k*0.004
        wx = GW/2 + GW*0.40*np.sin(TAU*tk); wy = GH/2 + GH*0.27*np.sin(2*TAU*tk) + 26*np.cos(TAU*tk)
        xi, yi = int(wx + 3*np.sin(k*1.7)), int(wy + 3*np.cos(k*2.3))
        if 0 <= yi < GH and 0 <= xi < GW:
            img[yi, xi] = max(img[yi, xi], 0.9*(1-k/26))

    # wordmark layer
    tw = T.shape[1]; tx, ty = (GW-tw)//2, (GH-TH_txt)//2
    shim = 0.86 + 0.12*np.sin(TAU*2*t + (np.arange(tw)/tw)*TAU)
    tlayer = T*shim[None, :]
    tmask = T > 0.35

    ox, oy, z, inside, shade, _ = bird_at(t)
    gap = ndimage.binary_dilation(inside, iterations=2)
    bg0 = np.zeros_like(shade)
    if z < 0:
        # behind: bird first, then the text with a thin dark gap around the letters
        place(img, shade, inside, ox, oy)
        tgap = ndimage.binary_dilation(tmask, iterations=2)
        place(img, np.zeros_like(tlayer), tgap & ~tmask, tx, ty)  # gap only reads where the bird is
        place(img, tlayer, tmask, tx, ty)
    else:
        # in front: text first, then a dark gap ring, then the bird
        place(img, tlayer, tmask, tx, ty)
        place(img, bg0, gap & ~inside, ox, oy)
        place(img, shade, inside, ox, oy)

    bits = img > TH
    if invert:
        fg, bg = (17,17,17), (250,250,250)
    else:
        fg, bg = (237,237,237), (12,12,12)
    out = np.empty((GH, GW, 3), np.uint8); out[:] = bg; out[bits] = fg
    return Image.fromarray(out).resize((GW*PX, GH*PX), Image.NEAREST)

if __name__ == "__main__":
    for name, inv in [("dark", False), ("light", True)]:
        frames = [frame(i/N, inv).convert("P", palette=Image.ADAPTIVE, colors=2) for i in range(N)]
        path = OUT+f"tern-hero-{name}.gif"
        frames[0].save(path, save_all=True, append_images=frames[1:], duration=40, loop=0, optimize=True, disposal=1)
        frame(0.06, inv).save(OUT+f"tern-hero-still-{name}.png")
        print(path, os.path.getsize(path))
