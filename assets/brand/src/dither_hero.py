# Renders the animated dither hero at the top of the README.
#
# Story (16 s loop): the wordmark sits alone in a live dither sky. A procedural
# 3D tern (lofted body, black cap, beak, tail streamers, wings jointed at the
# shoulder, elbow and wrist) flies in, loops behind the wordmark and back in
# front of it, banks into its turns, then glides into the logo's place and
# settles into the exact original mark. The sky stays alive and a shimmer runs
# across the mark until the mark fades and the tern flies in again.
# The lit frame is reduced to two tones with an 8x8 Bayer ordered dither.
#
# Requires: Python 3.10+, numpy, pillow, scipy, numba.
# Run:      python assets/brand/src/dither_hero.py
import os
import numpy as np
from PIL import Image
from scipy import ndimage
from numba import njit

HERE = os.path.dirname(os.path.abspath(__file__))
BR = os.path.join(HERE, "..") + "/"
OUT = BR

GW, GH = 600, 200            # dither grid
SS = 2                       # supersampling for the 3D render
PX = 2                       # output pixel size -> 1200 x 400
FPS = 20
DUR = 16.0                   # seconds per loop
N = int(FPS*DUR)
TAU = 2*np.pi
RW, RH = GW*SS, GH*SS
FOC = 820.0*SS/2             # focal length in render pixels
Z0 = 6.0                     # depth of the wordmark plane
CAM_Y = 0.25                 # camera height (looks slightly down on the bird)

# ------------------------------------------------------------------ dither
def bayer(n):
    if n == 1: return np.array([[0]])
    b = bayer(n//2)
    return np.block([[4*b, 4*b+2], [4*b+3, 4*b+1]])
B8 = (bayer(8)+0.5)/64.0
TH = np.tile(B8, (GH//8+1, GW//8+1))[:GH, :GW]

# ------------------------------------------------------------------ math
def rx(a):
    c, s = np.cos(a), np.sin(a); return np.array([[1,0,0],[0,c,-s],[0,s,c]])
def ry(a):
    c, s = np.cos(a), np.sin(a); return np.array([[c,0,s],[0,1,0],[-s,0,c]])
def rz(a):
    c, s = np.cos(a), np.sin(a); return np.array([[c,-s,0],[s,c,0],[0,0,1]])
def nrm(v): return v/np.linalg.norm(v)

# ------------------------------------------------------------------ bird mesh
# Local frame: x forward, y up, z to the side. Units: beak tip to tail base ~1.1.
NU, NV = 30, 18
XS = np.linspace(-0.50, 0.64, NU)
def radius(x):
    k = np.array([-0.50,-0.40,-0.25,-0.08, 0.10, 0.22, 0.29, 0.36, 0.42, 0.47, 0.55, 0.64])
    r = np.array([ 0.035,0.07, 0.115,0.140,0.132,0.098,0.090,0.094,0.082,0.042,0.018,0.002])
    return np.interp(x, k, r)
def ycen(x):
    return np.interp(x, [-0.5, -0.1, 0.2, 0.36, 0.64], [0.02, 0.0, 0.03, 0.08, 0.06])

def body_mesh(head_pitch):
    """Vertices, albedo, and triangles of body+head+beak. Head rotated for stabilisation."""
    V, A = [], []
    for x in XS:
        r = radius(x); yc = ycen(x)
        for j in range(NV):
            a = TAU*j/NV
            y, z = np.cos(a)*r*1.12, np.sin(a)*r*0.92
            p = np.array([x, yc + y, z])
            if x > 0.24:                              # head joint
                piv = np.array([0.24, 0.05, 0.0])
                p = piv + rz(head_pitch) @ (p - piv)
            alb = 0.96
            if 0.27 < x < 0.47 and np.cos(a) > -0.15: alb = 0.16      # black cap
            if x >= 0.47: alb = 0.50                                  # beak
            if -0.30 < x < 0.20 and np.cos(a) > 0.55: alb = 0.80      # pale grey mantle
            V.append(p); A.append(alb)
    T = []
    for i in range(NU-1):
        for j in range(NV):
            a = i*NV + j; b = i*NV + (j+1) % NV; c = a+NV; d = b+NV
            T += [(a, c, b), (b, c, d)]
    return np.array(V), np.array(A), np.array(T)

def strip(lead, trail, up, camber, alb):
    """Quad strip between a leading and trailing polyline, with camber."""
    V, A, T = [], [], []
    m = len(lead); nc = 4
    for i in range(m):
        for k in range(nc):
            w = k/(nc-1)
            p = lead[i]*(1-w) + trail[i]*w + up[i]*camber[i]*np.sin(np.pi*w)
            V.append(p); A.append(alb[i][k] if hasattr(alb[i], '__len__') else alb[i])
    for i in range(m-1):
        for k in range(nc-1):
            a = i*nc+k; b = a+1; c = a+nc; d = c+1
            T += [(a, c, b), (b, c, d)]
    return np.array(V), np.array(A), np.array(T)

def wing_mesh(s, phase, amp):
    """One wing. s=+1/-1 side. phase in [0,1): 0 = top of the upstroke."""
    ph = TAU*phase
    down = np.sin(ph) > 0                  # wing travelling down
    up_amt = max(0.0, -np.sin(ph))         # how much of the upstroke we are in
    alpha = np.radians(4 + 36*amp*np.cos(ph))                    # arm flap
    beta = np.radians(4 + 12*up_amt)                             # arm sweep back
    alpha_h = np.radians(18*amp*np.cos(ph - 0.9))                # hand lags the arm
    beta_h = np.radians(12 + 46*up_amt)                          # wrist folds on upstroke
    twist = np.radians(-10*np.sin(ph))                           # hand pitches with the stroke
    P0 = np.array([0.10, 0.08, s*0.09])
    la, lh = 0.42, 0.78
    Ra = rx(-s*alpha) @ ry(-s*beta)
    Rh = Ra @ rx(-s*alpha_h) @ ry(-s*beta_h)
    E = P0 + Ra @ np.array([0, 0, s*la])
    lead, trail, upv, camb, alb = [], [], [], [], []
    na, nh = 5, 10
    for i in range(na):
        w = i/(na-1)
        p = P0 + Ra @ np.array([0, 0, s*la*w])
        ch = 0.30 - 0.04*w
        lead.append(p); trail.append(p + Ra @ np.array([-ch, 0, 0]))
        upv.append(Ra @ np.array([0, 1, 0])); camb.append(0.035*ch/0.3)
        alb.append([0.82, 0.86, 0.90, 0.92])
    for i in range(1, nh):
        v = i/(nh-1)
        p = E + Rh @ np.array([0, 0, s*lh*v])
        ch = 0.26*(1-v)**0.85 + 0.015
        tw = twist*v
        cd = Rh @ np.array([-np.cos(tw), np.sin(tw), 0.0])
        # pointed tern hand: trailing edge sweeps toward the tip
        lead.append(p); trail.append(p + cd*ch + Rh @ np.array([0, 0, -s*0.10*v*ch]))
        upv.append(Rh @ np.array([0, 1, 0])); camb.append(0.03*ch/0.26)
        dark = 0.42 if v > 0.55 else 0.80            # dark outer primaries
        alb.append([dark, 0.84, 0.88, 0.90])
    return strip(lead, trail, upv, camb, alb)

def tail_mesh(spread):
    V, A, T = [], [], []
    base = np.array([-0.46, 0.03, 0.0])
    for s in (1, -1):
        lead = []; trail = []; upv = []; camb = []; alb = []
        for i in range(8):
            v = i/7
            x = -0.46 - 0.62*v
            zc = s*(0.02 + (0.10+0.08*spread)*v**1.2)
            wd = 0.07*(1-v)**1.4 + 0.004
            lead.append(np.array([x, 0.03 - 0.03*v, zc + s*wd*0.5]))
            trail.append(np.array([x, 0.03 - 0.03*v, zc - s*wd*0.5]))
            upv.append(np.array([0, 1, 0])); camb.append(0.0); alb.append(0.88)
        v_, a_, t_ = strip(lead, trail, upv, camb, alb)
        T.append(t_ + sum(len(x) for x in V)); V.append(v_); A.append(a_)
    return np.vstack(V), np.concatenate(A), np.vstack(T)

# ------------------------------------------------------------------ timeline (seconds)
T_GLIDE = 4.6     # last wingbeat ends, wings held raised for the landing
T_LAND = 5.6      # bird reaches the logo position
X0, X1 = 5.0, 5.9 # 3D bird dissolves into the flat mark
F0, F1 = 15.2, 16.0  # mark fades out before the next flight
BEATS = 11
def smooth(a, b, x):
    u = np.clip((x-a)/(b-a), 0, 1); return u*u*(3-2*u)

def bird_mesh(t):
    phase = (BEATS/T_GLIDE*min(t, T_GLIDE)) % 1.0
    amp = 1.0
    ph = TAU*phase
    head_pitch = -0.10*np.cos(ph)*(1-smooth(4.2, T_GLIDE, t))
    parts = [body_mesh(head_pitch), wing_mesh(1, phase, amp), wing_mesh(-1, phase, amp),
             tail_mesh(0.3 + 0.7*smooth(4.4, T_LAND, t))]
    V, A, T, off = [], [], [], 0
    for v, a, tr in parts:
        V.append(v); A.append(a); T.append(tr + off); off += len(v)
    V = np.vstack(V); A = np.concatenate(A); T = np.vstack(T)
    k = 1 - smooth(4.2, T_GLIDE, t)
    V = V @ rz(0.05*np.sin(ph)*k).T
    V[:, 1] += 0.035*np.cos(ph)*k
    return V, A, T

# ------------------------------------------------------------------ lockup layout
lk = Image.open(BR + "tern-lockup-on-dark.png")
LA = np.array(lk.split()[-1]).astype(float)/255.0       # 136 x 743, mark cols < 170
LK = 38*SS/56.0                                    # render px per lockup px
LKW, LKH = int(LA.shape[1]*LK), int(LA.shape[0]*LK)
LAr = np.array(Image.fromarray((LA*255).astype(np.uint8)).resize((LKW, LKH), Image.LANCZOS))/255.0
LX, LY = (RW-LKW)//2, (RH-LKH)//2
CUT = int(170*LK)
MARK = np.zeros((RH, RW)); TEXT = np.zeros((RH, RW))
MARK[LY:LY+LKH, LX:LX+CUT] = LAr[:, :CUT]
TEXT[LY:LY+LKH, LX+CUT:LX+LKW] = LAr[:, CUT:]
_my, _mx = np.where(MARK > 0.5)
MARK_BOX = (_mx.min(), _mx.max(), _my.min(), _my.max())

def to_world(px, py, z):
    return np.array([(px-RW/2)*z/FOC, -(py-RH/2)*z/FOC + CAM_Y, z])

# ------------------------------------------------------------------ flight path
Z_END = 4.0
DIVE = nrm(np.array([1.0, -0.62, 0.0]))
def ctrl(M):
    return np.array([[-9.0, 1.6, 11.0], [-2.5, 1.1, 9.6], [3.0, 0.55, 8.2], [3.0, -0.40, 5.2],
                     [0.6, -0.85, 4.5], [-1.9, -0.1, 4.6], [M[0]-1.6, M[1]+1.0, Z_END+0.15],
                     M, M + DIVE*1.2])
def catmull(P, u):
    n = len(P) - 3                      # segments between P[1]..P[-2]
    u = np.clip(u, 0, 1)*n; i = min(int(u), n-1); f = u - i
    p0, p1, p2, p3 = P[i], P[i+1], P[i+2], P[i+3]
    return 0.5*((2*p1) + (-p0+p2)*f + (2*p0-5*p1+4*p2-p3)*f*f + (-p0+3*p1-3*p2+p3)*f*f*f)
def u_of(t):
    s = np.clip(t/T_LAND, 0, 1)
    return 1 - (1-s)**1.7               # decelerates into the landing

M_WORLD = to_world((MARK_BOX[0]+MARK_BOX[1])/2, (MARK_BOX[2]+MARK_BOX[3])/2, Z_END)
CP = ctrl(M_WORLD)
def path(t):
    return catmull(CP, u_of(t))

def pose(t):
    e = 1e-3
    if t >= T_LAND - 0.02:
        f = DIVE
        up = nrm(np.array([0, 1.0, 0]) - np.dot([0, 1.0, 0], f)*f)
        return path(T_LAND), np.stack([f, up, np.cross(f, up)], axis=1)
    p = path(t); v = (path(t+e) - path(t-e))/(2*e); a = (path(t+e) - 2*p + path(t-e))/(e*e)
    f = nrm(v)
    k = 1 - smooth(4.4, T_LAND, t)
    f = nrm(f*(1-(1-k)) + DIVE*(1-k)) if k < 1 else f
    a_lat = a - np.dot(a, f)*f
    up = np.array([0.0, 9.0, 0.0]) + a_lat*0.0018*k
    up = nrm(up - np.dot(up, f)*f)
    return p, np.stack([f, up, np.cross(f, up)], axis=1)

S_FLY = 1.05
S_END = 1.0       # calibrated below so the landed bird matches the mark's size
def scale_at(t):
    return S_FLY + (S_END - S_FLY)*smooth(3.4, T_LAND, t)

# ------------------------------------------------------------------ raster
@njit
def raster(sx, sy, sz, inten, tris, zbuf, cbuf):
    H, W = zbuf.shape
    for t in range(tris.shape[0]):
        i0, i1, i2 = tris[t, 0], tris[t, 1], tris[t, 2]
        x0, y0, x1, y1, x2, y2 = sx[i0], sy[i0], sx[i1], sy[i1], sx[i2], sy[i2]
        if sz[i0] <= 0.1 or sz[i1] <= 0.1 or sz[i2] <= 0.1: continue
        den = (y1-y2)*(x0-x2) + (x2-x1)*(y0-y2)
        if abs(den) < 1e-9: continue
        minx = max(int(min(x0, x1, x2)), 0); maxx = min(int(max(x0, x1, x2))+1, W-1)
        miny = max(int(min(y0, y1, y2)), 0); maxy = min(int(max(y0, y1, y2))+1, H-1)
        for py in range(miny, maxy+1):
            for px in range(minx, maxx+1):
                fx = px + 0.5; fy = py + 0.5
                w0 = ((y1-y2)*(fx-x2) + (x2-x1)*(fy-y2))/den
                w1 = ((y2-y0)*(fx-x2) + (x0-x2)*(fy-y2))/den
                w2 = 1.0 - w0 - w1
                if w0 < 0 or w1 < 0 or w2 < 0: continue
                z = w0*sz[i0] + w1*sz[i1] + w2*sz[i2]
                if z < zbuf[py, px]:
                    zbuf[py, px] = z
                    cbuf[py, px] = w0*inten[i0] + w1*inten[i1] + w2*inten[i2]

def vertex_normals(V, T):
    n = np.zeros_like(V)
    fn = np.cross(V[T[:, 1]] - V[T[:, 0]], V[T[:, 2]] - V[T[:, 0]])
    for k in range(3): np.add.at(n, T[:, k], fn)
    l = np.linalg.norm(n, axis=1, keepdims=True); l[l == 0] = 1
    return n/l

LIGHT = nrm(np.array([-0.45, 0.85, -0.55]))     # from upper left, camera side

def render_bird(t, scale=None, offset=(0.0, 0.0)):
    V, A, T = bird_mesh(t)
    p, R = pose(t)
    sc = scale_at(t) if scale is None else scale
    W_ = (V*sc) @ R.T + p
    W_[:, 0] += offset[0]*smooth(3.4, T_LAND, t); W_[:, 1] += offset[1]*smooth(3.4, T_LAND, t)
    Nl = vertex_normals(V, T) @ R.T
    cam = np.array([0.0, CAM_Y, 0.0])
    view = cam - W_; view /= np.linalg.norm(view, axis=1, keepdims=True)
    flip = np.sum(Nl*view, axis=1) < 0                      # two-sided lighting
    Nl[flip] *= -1
    lam = np.clip(Nl @ LIGHT, 0, 1)
    rim = (1 - np.clip(np.sum(Nl*view, axis=1), 0, 1))**2.5
    depth_dim = np.clip(1.15 - (W_[:, 2] - (Z0 - 2.5))/5.0*0.35, 0.72, 1.1)
    inten = (A*(0.34 + 0.86*lam) + 0.25*rim)*depth_dim
    Y = W_[:, 1] - CAM_Y
    sx = RW/2 + FOC*W_[:, 0]/W_[:, 2]
    sy = RH/2 - FOC*Y/W_[:, 2]
    zbuf = np.full((RH, RW), np.inf); cbuf = np.zeros((RH, RW))
    raster(sx, sy, W_[:, 2].copy(), inten, T.astype(np.int64), zbuf, cbuf)
    return zbuf, cbuf, p

# ------------------------------------------------------------------ landing calibration
def _calibrate():
    global S_END, OFFSET
    OFFSET = (0.0, 0.0)
    z, _, _ = render_bird(T_LAND, scale=1.0)
    ys, xs = np.where(np.isfinite(z))
    S_END = (MARK_BOX[1]-MARK_BOX[0])*0.98/(xs.max()-xs.min())
    z, _, _ = render_bird(T_LAND, scale=S_END)
    ys, xs = np.where(np.isfinite(z))
    dx = (MARK_BOX[0]+MARK_BOX[1])/2 - (xs.min()+xs.max())/2
    dy = (MARK_BOX[2]+MARK_BOX[3])/2 - (ys.min()+ys.max())/2
    OFFSET = (dx*Z_END/FOC, -dy*Z_END/FOC)
OFFSET = (0.0, 0.0)

# ------------------------------------------------------------------ atmosphere
yy, xx = np.mgrid[0:GH, 0:GW].astype(float)
def pnoise(x, y, t, seed):
    rng = np.random.default_rng(seed); v = np.zeros_like(x)
    for _ in range(6):
        fx, fy = rng.uniform(0.01, 0.05, 2)*rng.choice([-1, 1], 2)
        ph = rng.uniform(0, TAU); sp = rng.integers(1, 3)*rng.choice([-1, 1])
        v += np.sin(fx*x + fy*y + ph + sp*TAU*t)
    return v/6
rng = np.random.default_rng(7)
NP = 120
p0x = rng.uniform(0, GW, NP); p0y = rng.uniform(0, GH, NP)
pv = rng.integers(1, 4, NP); pb = rng.uniform(0.3, 0.9, NP); pl = rng.integers(1, 6, NP)

def frame(sec, invert=False):
    t = sec/DUR                                   # loop phase for the sky
    fog = pnoise(xx, yy, t, 3)*0.5 + pnoise(xx*1.7, yy*1.7, t, 11)*0.5
    fog = np.clip((fog - 0.05)*0.40, 0, 0.20)
    vig = np.clip(1.15 - (((xx-GW/2)/(GW*0.62))**2 + ((yy-GH/2)/(GH*0.9))**2), 0, 1)
    img = fog*vig
    for i in range(NP):
        x = (p0x[i] - pv[i]*GW*t) % GW; y = p0y[i] + 2*np.sin(TAU*(t + i*0.13))
        xi, yi = int(x), int(y)
        if 0 <= yi < GH:
            for k in range(pl[i]):
                img[yi, (xi+k) % GW] = max(img[yi, (xi+k) % GW], pb[i]*(1 - k/pl[i])*0.7)

    hi = np.zeros((RH, RW)); cov = np.zeros((RH, RW), bool)
    tmask = TEXT > 0.4
    tshim = 0.86 + 0.08*np.sin(TAU*sec/4.0 + np.linspace(0, TAU, RW))[None, :]
    w = smooth(X0, X1, sec)                       # bird -> mark
    fade = 1 - smooth(F0, F1, sec)                # mark fades before the loop restarts
    if sec < X1:
        zbuf, cbuf, _ = render_bird(min(sec, T_LAND), offset=OFFSET)
        bird = np.isfinite(zbuf)
    else:
        bird = np.zeros((RH, RW), bool); zbuf = None; cbuf = None
    front = bird & (zbuf < Z0) if zbuf is not None else bird
    behind = bird & ~front
    if behind.any():
        hi[behind] = cbuf[behind]; cov |= behind
        tgap = ndimage.binary_dilation(tmask, iterations=3) & behind & ~tmask
        hi[tgap] = 0; cov |= tgap
    hi[tmask] = (TEXT*tshim)[tmask]; cov |= tmask
    if front.any():
        fgap = ndimage.binary_dilation(front, iterations=3) & ~front & tmask
        hi[fgap] = 0
        hi[front] = cbuf[front]*(1-w); cov |= front
    if w > 0 and fade > 0:
        mmask = MARK > 0.4
        # shimmer: a soft light band sweeps diagonally across the mark every 2.6 s
        hold = max(0.0, sec - X1)
        pos = ((hold/2.6) % 1.0)*1.6 - 0.3
        gx = (np.arange(RW) - MARK_BOX[0])/(MARK_BOX[1]-MARK_BOX[0])
        gy = (np.arange(RH) - MARK_BOX[2])/(MARK_BOX[3]-MARK_BOX[2])
        d = gx[None, :]*0.8 + gy[:, None]*0.45 - pos
        band = np.exp(-(d/0.07)**2) - 0.5*np.exp(-((d+0.12)/0.05)**2)
        mval = np.clip(0.90 + 0.25*band*(hold > 0), 0, 1)*MARK
        m_on = mmask
        prev = hi[m_on]
        hi[m_on] = np.maximum(prev*(1-w), mval[m_on]*w*fade)
        cov |= m_on
    # down to the dither grid
    hs = hi.reshape(GH, SS, GW, SS).mean(axis=(1, 3))
    cs = cov.reshape(GH, SS, GW, SS).mean(axis=(1, 3))
    img = img*(1 - cs) + hs
    bits = img > TH
    fg, bg = ((17, 17, 17), (250, 250, 250)) if invert else ((237, 237, 237), (12, 12, 12))
    out = np.empty((GH, GW, 3), np.uint8); out[:] = bg; out[bits] = fg
    return Image.fromarray(out).resize((GW*PX, GH*PX), Image.NEAREST)

_calibrate()

if __name__ == "__main__":
    for name, inv in [("dark", False), ("light", True)]:
        frames = [frame(i/FPS, inv).convert("P", palette=Image.ADAPTIVE, colors=2) for i in range(N)]
        path_ = OUT + f"tern-hero-{name}.gif"
        frames[0].save(path_, save_all=True, append_images=frames[1:], duration=int(1000/FPS), loop=0,
                       optimize=True, disposal=1)
        frame(9.0, inv).save(OUT + f"tern-hero-still-{name}.png")
        print(path_, os.path.getsize(path_))
