# Renders the animated dither hero used at the top of the README.
# Requires: Python 3.10+, numpy, pillow, scipy.  Run: python assets/brand/src/dither_hero.py
import numpy as np, sys
from PIL import Image
from scipy import ndimage

import os
BR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..") + "/"
OUT = BR
GW, GH = 600, 180          # dither grid
PX = 2                     # output pixel size -> 960x340
N = 60                     # frames, 3 s at 20 fps, seamless loop
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
MH = 126
MW = int(mark.width*MH/mark.height)
mark_hi = mark.resize((MW*4, MH*4), Image.LANCZOS)

# ---- wordmark from lockup alpha (text part only)
lk = Image.open(BR+"tern-lockup-on-dark.png")
a = np.array(lk.split()[-1])
txt = Image.fromarray(a[:, 180:])
bbox = txt.getbbox(); txt = txt.crop(bbox)
TH_txt = 36
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
pv = rng.integers(1, 4, NP)          # laps per loop -> seamless
pb = rng.uniform(0.35, 1.0, NP)
pl = rng.integers(1, 7, NP)          # streak length

def frame(t, invert=False):
    # fog
    fog = pnoise(xx, yy, t, 3)*0.5 + pnoise(xx*1.7, yy*1.7, t, 11)*0.5
    fog = np.clip((fog-0.05)*0.42, 0, 0.22)
    # vignette keeps edges calm
    vig = np.clip(1.15 - (((xx-GW*0.3)/(GW*0.75))**2 + ((yy-GH/2)/(GH*0.9))**2), 0, 1)
    img = fog*vig

    # particles drifting left, behind the bird
    for i in range(NP):
        x = (p0x[i] - pv[i]*GW*t) % GW
        y = p0y[i] + 2*np.sin(TAU*(t+i*0.13))
        xi, yi = int(x), int(y)
        if 0 <= yi < GH:
            for k in range(pl[i]):
                xk = (xi+k) % GW
                img[yi, xk] = max(img[yi, xk], pb[i]*(1-k/pl[i])*0.85)

    # bird: bob + tiny tilt, periodic
    bob = 4.0*np.sin(TAU*t)
    ang = 1.6*np.sin(TAU*t + 0.6)
    m = mark_hi.rotate(ang, resample=Image.BICUBIC, expand=False)
    m = np.array(m.resize((MW, MH), Image.LANCZOS))/255.0
    inside = m > 0.5
    dist = ndimage.distance_transform_edt(inside)
    dist = np.clip(dist/6.0, 0, 1)
    my, mx = np.mgrid[0:MH, 0:MW]
    # light sweep across the body
    sweep = np.exp(-(((mx+my*0.6) - (MW+MH)*((t*1.0) % 1.0)*1.3 + 30)/18.0)**2)
    shade = 0.74 + 0.22*(1-dist) + 0.30*sweep
    shade = np.where(inside, np.clip(shade, 0, 1), 0)
    ox, oy = 34, int(round((GH-MH)/2 + bob))
    sub = img[max(oy,0):oy+MH, ox:ox+MW]
    sh = shade[max(-oy,0):max(-oy,0)+sub.shape[0], :sub.shape[1]]
    ins = inside[max(-oy,0):max(-oy,0)+sub.shape[0], :sub.shape[1]]
    sub[ins] = sh[ins]
    # soft halo just outside the silhouette
    halo = ndimage.distance_transform_edt(~inside)
    halo = np.exp(-halo/5.0)*0.14*(0.8+0.2*np.sin(TAU*t))
    hs = halo[max(-oy,0):max(-oy,0)+sub.shape[0], :sub.shape[1]]
    sub[:] = np.maximum(sub, hs)

    # wordmark, static with a slow shimmer
    tx, ty = 34+MW+22, (GH-TH_txt)//2 + 6
    tw = T.shape[1]; tw = min(tw, GW-tx-6)
    shim = 0.86 + 0.12*np.sin(TAU*t + (np.arange(tw)/tw)*TAU)
    region = img[ty:ty+TH_txt, tx:tx+tw]
    tv = T[:, :tw]*shim[None, :]
    img[ty:ty+TH_txt, tx:tx+tw] = np.maximum(region*(1-T[:, :tw]), tv)

    bits = img > TH
    if invert:
        fg, bg = (17,17,17), (250,250,250)
    else:
        fg, bg = (237,237,237), (12,12,12)
    out = np.empty((GH, GW, 3), np.uint8); out[:] = bg; out[bits] = fg
    return Image.fromarray(out).resize((GW*PX, GH*PX), Image.NEAREST)

for name, inv in [("dark", False), ("light", True)]:
    frames = [frame(i/N, inv).convert("P", palette=Image.ADAPTIVE, colors=2) for i in range(N)]
    path = OUT+f"tern-hero-{name}.gif"
    frames[0].save(path, save_all=True, append_images=frames[1:], duration=50, loop=0, optimize=True, disposal=1)
    frame(0.25, inv).save(OUT+f"tern-hero-still-{name}.png")
    print(path)
