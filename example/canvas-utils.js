/**
 * Canvas utility functions for animation examples
 */

/**
 * Draw a shape that morphs from square to circle
 * @param {CanvasRenderingContext2D} ctx - Canvas context
 * @param {number} centerX - Center X position
 * @param {number} centerY - Center Y position
 * @param {number} size - Size of the shape
 * @param {number} cornerRadius - Corner radius (0 = square, size/2 = circle)
 * @param {string} color - Fill color
 */
export function drawRoundedSquare(ctx, centerX, centerY, size, cornerRadius, color) {
  const halfSize = size / 2
  const x = centerX - halfSize
  const y = centerY - halfSize

  // Clamp corner radius to valid range
  const radius = Math.min(cornerRadius, halfSize)

  ctx.beginPath()
  ctx.moveTo(x + radius, y)
  ctx.lineTo(x + size - radius, y)
  ctx.quadraticCurveTo(x + size, y, x + size, y + radius)
  ctx.lineTo(x + size, y + size - radius)
  ctx.quadraticCurveTo(x + size, y + size, x + size - radius, y + size)
  ctx.lineTo(x + radius, y + size)
  ctx.quadraticCurveTo(x, y + size, x, y + size - radius)
  ctx.lineTo(x, y + radius)
  ctx.quadraticCurveTo(x, y, x + radius, y)
  ctx.closePath()

  ctx.fillStyle = color
  ctx.fill()
}

/**
 * Easing function for smooth animation (cubic ease in-out)
 * @param {number} t - Progress value from 0 to 1
 * @returns {number} Eased value from 0 to 1
 */
export function easeInOutCubic(t) {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
}