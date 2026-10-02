#' Read an image as an array
#'
#' Reads the pixels of one image (a CZI scene, an ND2 position, a LIF series, a plate field, a
#' whole-slide image, ...) into an R array of dimensions `x, y, z, c, t`, the reverse of the
#' Python bindings' `TCZYX` and OME's `XYZCT` order, as in EBImage and imager. Interleaved RGB
#' images get a sample dimension after `y`: `x, y, s, z, c, t`. `img[x, y, z, c, t]` is the
#' pixel at column `x`, row `y`; `t(img[, , 1, 1, 1])` is the usual row-by-column matrix.
#'
#' Pixels of 8- and 16-bit and signed 32-bit images are returned as integers (R has no unsigned
#' 16-bit type; values are exact), unsigned 32-bit and floating-point images as doubles.
#'
#' Only the planes asked for are decoded. `level` reads a downsampled level of a pyramidal image
#' (see [openreadout_levels()]) and `region` a rectangle; formats stored in tiles (CZI, tiled and
#' whole-slide TIFF, VSI, Imaris, OME-Zarr) decode only the tiles the region touches.
#'
#' @param x A path or an [openreadout_open()] file.
#' @param image Which image (1 = the first; `openreadout_info(x)$images`).
#' @param c,z,t Channels, z planes and time points to read (1-based vectors); `NULL` reads all.
#' @param level Pyramid level: 0 is full resolution, higher levels are downsampled (the `level`
#'   numbers of [openreadout_levels()]).
#' @param region `c(x, y, width, height)`: a rectangle of the level, `x` and `y` of its top-left
#'   pixel counted from 1. `NULL` reads whole planes.
#' @param drop Drop the dimensions of extent 1 other than `x` and `y`.
#' @return An integer or double array with attributes `physical_size` (µm per pixel in `x`, `y`
#'   and the z step, at this level; `NA` when unknown), `time_increment_s`, `pixel_type` (the
#'   stored type, e.g. `"uint16"`), `image`, `level` and `region`. Dimension names: `x`, `y`,
#'   (`s`), `z`, `c` (named after the channels), `t`.
#' @seealso [openreadout_read_plane()] for one plane as a matrix.
#' @examples
#' \dontrun{
#' img <- openreadout_read_image("run42.czi", image = 2, c = 1)
#' dim(img)                       # x y z c t
#' attr(img, "physical_size")     # µm
#' mip <- apply(img[, , , 1, 1], c(1, 2), max)   # maximum projection over z
#' thumb <- openreadout_read_image("slide.svs", level = 2)
#' tile <- openreadout_read_image("slide.svs", region = c(20001, 15001, 512, 512))
#' }
#' @export
openreadout_read_image <- function(x, image = 1, c = NULL, z = NULL, t = NULL, level = 0,
                          region = NULL, drop = FALSE) {
  .with_file(x, function(h) {
    info <- .info_cached(h)
    im_i <- .index0(image, "image")
    if (im_i >= length(info$images)) {
      .openreadout_abort(sprintf("image %d does not exist: the file has %d image(s)", image, length(info$images)))
    }
    im <- info$images[[im_i + 1]]
    pick <- function(v, n, what) {
      if (is.null(v)) {
        return(seq_len(n))
      }
      if (!is.numeric(v) || length(v) == 0 || anyNA(v) || any(v != round(v)) || any(v < 1) || any(v > n)) {
        .openreadout_abort(sprintf("%s must be whole numbers from 1 to %d (image %d has %d)", what, n, image, n),
                  call = sys.call(-2))
      }
      as.integer(v)
    }
    cs <- pick(c, im$size_c, "c")
    zs <- pick(z, .level_size_z(im, level), "z")
    ts <- pick(t, im$size_t, "t")
    lv <- .level_row(im, level)
    reg <- NULL
    if (!is.null(region)) {
      if (!is.numeric(region) || length(region) != 4 || anyNA(region) || any(region != round(region)) ||
          region[1] < 1 || region[2] < 1 || region[3] < 1 || region[4] < 1) {
        .openreadout_abort("region must be c(x, y, width, height): whole numbers, x and y from 1, width and height at least 1")
      }
      if (region[1] + region[3] - 1 > lv$size_x || region[2] + region[4] - 1 > lv$size_y) {
        .openreadout_abort(sprintf("region c(%s) extends beyond level %d (%d x %d pixels)",
                          paste(region, collapse = ", "), level, lv$size_x, lv$size_y))
      }
      reg <- as.integer(c(region[1] - 1, region[2] - 1, region[3], region[4]))
    }
    w <- if (is.null(reg)) lv$size_x else reg[3]
    hgt <- if (is.null(reg)) lv$size_y else reg[4]
    spp <- max(1L, im$samples_per_pixel %||% 1L)
    n <- as.double(w) * hgt * spp * length(zs) * length(cs) * length(ts)
    bytes <- n * if (im$pixel_type %in% c("uint32", "float", "double")) 8 else 4
    limit <- getOption("openreadout.max_bytes", 4 * 1024^3)
    if (bytes > limit) {
      .openreadout_abort(sprintf(paste0(
        "reading %s would need %.1f GiB of R memory (limit %.1f GiB, options(openreadout.max_bytes)); ",
        "select planes (c, z, t), a pyramid level (openreadout_levels()) or a region"),
        if (is.null(reg)) "the whole selection" else "this region", bytes / 1024^3, limit / 1024^3))
    }
    grid <- expand.grid(z = zs, c = cs, t = ts)
    arr <- rs_read_planes(.ptr(h), im_i, grid$c - 1L, grid$z - 1L, grid$t - 1L,
                          as.integer(level), reg)
    # The pixels come back as one vector with the plane geometry in attributes. Only primitives
    # touch it until it is shaped: passing it to a closure (inherits(), .ic()) would leave it
    # shared, and dim<- would then copy every pixel.
    if (is.list(arr)) .ic(arr)
    res <- list(width = attr(arr, "openreadout_width"), height = attr(arr, "openreadout_height"),
                samples = attr(arr, "openreadout_samples"), pixel_type = attr(arr, "openreadout_pixel_type"))
    attributes(arr) <- NULL
    sdim <- if (res$samples > 1) res$samples else NULL
    dim(arr) <- c(res$width, res$height, sdim, length(zs), length(cs), length(ts))
    chan <- vapply(im$channels, function(ch) ch$name %||% NA_character_, "")
    cnames <- if (length(chan) == im$size_c && !anyNA(chan) && all(nzchar(chan))) chan[cs] else NULL
    dn <- list(x = NULL, y = NULL)
    if (!is.null(sdim)) {
      dn <- c(dn, list(s = if (res$samples == 3) c("R", "G", "B") else NULL))
    }
    dn <- c(dn, list(z = NULL, c = cnames, t = NULL))
    dimnames(arr) <- dn
    if (drop) {
      keep <- c(TRUE, TRUE, dim(arr)[-(1:2)] > 1)
      if (!all(keep)) {
        dn2 <- dimnames(arr)[keep]
        d2 <- dim(arr)[keep]
        dimnames(arr) <- NULL
        dim(arr) <- d2
        dimnames(arr) <- dn2
      }
    }
    ps <- im$physical_size
    num <- function(v) if (is.null(v)) NA_real_ else as.numeric(v)
    attr(arr, "physical_size") <- c(x = num(ps$x) * lv$downsample_x, y = num(ps$y) * lv$downsample_y,
                                    z = num(ps$z))
    attr(arr, "physical_size_unit") <- "um"
    attr(arr, "time_increment_s") <- num(im$time_increment_s)
    attr(arr, "pixel_type") <- res$pixel_type
    attr(arr, "image") <- image
    attr(arr, "level") <- level
    attr(arr, "region") <- region
    arr
  })
}

#' Read one plane as a matrix
#'
#' `openreadout_read_plane()` is [openreadout_read_image()] for a single `(c, z, t)` plane: a matrix `[x, y]`, or
#' an array `[x, y, s]` for RGB images, with the same attributes.
#'
#' @inheritParams openreadout_read_image
#' @param c,z,t The plane (one channel, z plane and time point; 1-based).
#' @return An integer or double matrix (`[x, y]`) or, for RGB, an array `[x, y, s]`.
#' @export
openreadout_read_plane <- function(x, image = 1, c = 1, z = 1, t = 1, level = 0, region = NULL) {
  for (v in list(c = c, z = z, t = t)) {
    if (length(v) != 1) .openreadout_abort("c, z and t must each be a single index; use openreadout_read_image() for stacks")
  }
  arr <- openreadout_read_image(x, image = image, c = c, z = z, t = t, level = level, region = region)
  keep <- names(dimnames(arr)) %in% c("x", "y", "s")
  at <- attributes(arr)[c("physical_size", "physical_size_unit", "time_increment_s", "pixel_type",
                          "image", "level", "region")]
  d2 <- dim(arr)[keep]
  dn2 <- dimnames(arr)[keep]
  attributes(arr) <- NULL
  dim(arr) <- d2
  dimnames(arr) <- dn2
  attributes(arr) <- c(attributes(arr), at[!vapply(at, is.null, logical(1))])
  arr
}

#' Pyramid levels of an image
#'
#' @inheritParams openreadout_read_image
#' @return A data frame: `level` (pass it as `level` to [openreadout_read_image()]), `size_x`, `size_y`,
#'   `downsample_x`, `downsample_y`, and the stored tile size where the level is tiled. Images
#'   without a pyramid have one row (level 0).
#' @export
openreadout_levels <- function(x, image = 1) {
  .with_file(x, function(h) {
    info <- .info_cached(h)
    i <- .index0(image, "image")
    if (i >= length(info$images)) {
      .openreadout_abort(sprintf("image %d does not exist: the file has %d image(s)", image, length(info$images)))
    }
    im <- info$images[[i + 1]]
    .records(.levels_of(im))
  })
}

.levels_of <- function(im) {
  lv <- im$resolution_levels
  if (length(lv) == 0) {
    lv <- list(list(level = 0L, size_x = im$size_x, size_y = im$size_y, downsample_x = 1, downsample_y = 1))
  }
  lv
}

.level_row <- function(im, level) {
  if (!is.numeric(level) || length(level) != 1 || is.na(level) || level < 0 || level != round(level)) {
    .openreadout_abort("level must be a single whole number >= 0 (0 = full resolution)", call = sys.call(-2))
  }
  for (l in .levels_of(im)) {
    if (l$level == level) return(l)
  }
  .openreadout_abort(sprintf("level %d does not exist: this image has levels %s (openreadout_levels())", level,
                    paste(vapply(.levels_of(im), function(l) l$level, 0), collapse = ", ")),
            call = sys.call(-2))
}

.level_size_z <- function(im, level) {
  for (l in .levels_of(im)) {
    if (l$level == level && !is.null(l$size_z)) return(l$size_z)
  }
  im$size_z
}
