# Notices

The xeibe source code in this package is CC0-1.0, like the rest of the xeibe
workspace. Two parts are not:

## EPSG Dataset

The package embeds data extracted from the **EPSG Geodetic Parameter
Dataset**, published by the International Association of Oil and Gas
Producers (IOGP) at <https://epsg.org/>. `xeibe_proj.EPSG_VERSION` gives the
release.

**Ownership of the EPSG Dataset by IOGP is hereby acknowledged.**

The EPSG Dataset is used under the EPSG Dataset Terms of Use,
<https://epsg.org/terms-of-use.html>, which require that IOGP's ownership is
acknowledged in any onward publication or transmission, that anyone the data
is passed on to is informed of the Terms of Use (which is what this section
is for), that the data is not distributed for profit, and that modified data
is not attributed to the EPSG Dataset. How the data was extracted is
described in `crates/xeibe-crs/EPSG-NOTICE.md` in the xeibe repository.

## PROJ

Some of the code in this package (in `xeibe-reproject`) is ported from PROJ 9.8.1
(<https://proj.org>, <https://github.com/OSGeo/PROJ>). Each ported module says
so at the top, with the PROJ source file it follows. The grid name table in
`xeibe-reproject`'s `src/grid/catalog.rs` is generated from PROJ's data
(`data/sql/grid_alternatives*.sql`). PROJ's licence applies to those parts:

```text
All source, data files and other contents of the PROJ package are
available under the following terms.  Note that the PROJ 4.3 and earlier
was "public domain" as is common with US government work, but apparently
this is not a well defined legal term in many countries. Frank Warmerdam placed
everything under the following MIT style license because he believed it is
effectively the same as public domain, allowing anyone to use the code as
they wish, including making proprietary derivatives.

Initial PROJ 4.3 public domain code was put as Frank Warmerdam as copyright
holder, but he didn't mean to imply he did the work. Essentially all work was
done by Gerald Evenden.

Copyright information can be found in source files.

 --------------

 Permission is hereby granted, free of charge, to any person obtaining a
 copy of this software and associated documentation files (the "Software"),
 to deal in the Software without restriction, including without limitation
 the rights to use, copy, modify, merge, publish, distribute, sublicense,
 and/or sell copies of the Software, and to permit persons to whom the
 Software is furnished to do so, subject to the following conditions:

 The above copyright notice and this permission notice shall be included
 in all copies or substantial portions of the Software.

 THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
 OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
 THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 DEALINGS IN THE SOFTWARE.
```

The Transverse Mercator code follows PROJ's Poder/Engsager implementation,
which PROJ credits to Knud Poder and Karsten Engsager (used by permission of
the Danish Kort og Matrikelstyrelsen), with the auxiliary-latitude series of
C. F. F. Karney, "On auxiliary latitudes", Survey Review 56, 165–180 (2024).
