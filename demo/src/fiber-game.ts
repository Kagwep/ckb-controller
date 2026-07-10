// A minimal canvas shooter whose every shot is a Fiber micropayment.
//
// The engine is deliberately decoupled from the payment rail: it calls
// `opts.tryShoot()` when the player fires. That returns whether a shot is
// affordable (and, in the wiring layer, kicks off the off-chain pay in the
// background). Gameplay never awaits payment finality — optimistic fire-and-
// forget, exactly the "don't gate play on payment" rule from the Fiber-central
// plan. Aim with the mouse, fire with click or Space.

export interface GameOpts {
  canvas: HTMLCanvasElement;
  /**
   * Fire one shot's micropayment. Return true if the shot is affordable (and
   * has been paid for in the background); false when the budget is exhausted,
   * which ends the game. Must NOT await the network — return synchronously.
   */
  tryShoot: () => boolean;
  /** Called whenever the score changes, so the HUD can refresh. */
  onScore: (score: number) => void;
  /** Called once when the game ends (budget out, or the player ends it). */
  onOver: (result: { score: number; reason: string }) => void;
}

export interface GameHandle {
  /** End the game early (e.g. the player clicks "End game"). */
  stop: () => void;
}

interface Bullet {
  x: number;
  y: number;
}

const POINTS_PER_HIT = 10;

export function runGame(opts: GameOpts): GameHandle {
  const { canvas } = opts;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas 2d context unavailable");
  const W = canvas.width;
  const H = canvas.height;

  let playerX = W / 2;
  const bullets: Bullet[] = [];
  const boss = { x: W / 2, y: 56, w: 96, h: 30, dir: 1, hp: 20, maxHp: 20 };
  let score = 0;
  let running = true;

  const clampX = (x: number) => Math.max(18, Math.min(W - 18, x));

  const onMove = (e: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    playerX = clampX(((e.clientX - rect.left) * W) / rect.width);
  };
  const fire = () => {
    if (!running) return;
    if (!opts.tryShoot()) {
      end("out of budget");
      return;
    }
    bullets.push({ x: playerX, y: H - 42 });
  };
  const onClick = () => fire();
  const onKey = (e: KeyboardEvent) => {
    if (e.code === "Space") {
      e.preventDefault();
      fire();
    }
  };

  canvas.addEventListener("mousemove", onMove);
  canvas.addEventListener("click", onClick);
  window.addEventListener("keydown", onKey);

  function cleanup() {
    canvas.removeEventListener("mousemove", onMove);
    canvas.removeEventListener("click", onClick);
    window.removeEventListener("keydown", onKey);
  }

  function end(reason: string) {
    if (!running) return;
    running = false;
    cleanup();
    opts.onOver({ score, reason });
  }

  function step() {
    if (!running) return;

    boss.x += boss.dir * 2.2;
    if (boss.x < boss.w / 2 || boss.x > W - boss.w / 2) boss.dir *= -1;

    for (let i = bullets.length - 1; i >= 0; i--) {
      const b = bullets[i];
      b.y -= 9;
      if (b.y < 0) {
        bullets.splice(i, 1);
        continue;
      }
      const hit =
        b.x > boss.x - boss.w / 2 &&
        b.x < boss.x + boss.w / 2 &&
        b.y < boss.y + boss.h &&
        b.y > boss.y;
      if (hit) {
        bullets.splice(i, 1);
        score += POINTS_PER_HIT;
        boss.hp -= 1;
        opts.onScore(score);
        if (boss.hp <= 0) boss.hp = boss.maxHp; // heal & keep going
      }
    }

    render();
    requestAnimationFrame(step);
  }

  function render() {
    if (!ctx) return;
    ctx.fillStyle = "#0b0f1a";
    ctx.fillRect(0, 0, W, H);

    // boss + hp bar
    ctx.fillStyle = "#e0508a";
    ctx.fillRect(boss.x - boss.w / 2, boss.y, boss.w, boss.h);
    ctx.fillStyle = "#2a3350";
    ctx.fillRect(boss.x - boss.w / 2, boss.y - 8, boss.w, 4);
    ctx.fillStyle = "#7ee0a0";
    ctx.fillRect(boss.x - boss.w / 2, boss.y - 8, (boss.w * boss.hp) / boss.maxHp, 4);

    // player ship
    ctx.fillStyle = "#5aa0ff";
    ctx.beginPath();
    ctx.moveTo(playerX, H - 46);
    ctx.lineTo(playerX - 14, H - 20);
    ctx.lineTo(playerX + 14, H - 20);
    ctx.closePath();
    ctx.fill();

    // bullets
    ctx.fillStyle = "#ffd060";
    for (const b of bullets) ctx.fillRect(b.x - 2, b.y - 8, 4, 8);
  }

  requestAnimationFrame(step);
  render();

  return { stop: () => end("ended by player") };
}
