# chiripa 🍀

**[chiripa.mx](https://chiripa.mx)** — números del Melate por pura estadística.

🌐 **En vivo:** [chiripa.mx](https://chiripa.mx) · espejo en [chiripa.fly.dev](https://chiripa.fly.dev)

Un binario de Rust que descarga el [histórico oficial de resultados](https://pronosticos.gob.mx/Documentos/Historicos/Melate.csv)
de Lotería Nacional, lo respalda en un bucket de [Tigris](https://www.tigrisdata.com/),
y sirve una página donde 8 estrategias estadísticas generan apuestas en tu
navegador: frecuencias, retrasos, muestreo ponderado, posterior de Dirichlet,
co-ocurrencia de Markov, y las combinaciones impopulares de Stern & Cover
(*"Maximum Entropy and the Lottery"*, JASA 1989). Con las ecuaciones en MathML,
porque podemos.

## ⚠️ Disclaimer

**Esto es pura diversión y no nos hacemos responsables de absolutamente nada.**
Sitio no oficial, sin afiliación alguna con Lotería Nacional para la Asistencia
Pública ni con el juego Melate®. Ninguna "estrategia" predice nada: toda
combinación de 6 números tiene exactamente la misma probabilidad de ganar —
**1 en 32,468,436**. Si juegas, juega con moderación y por gusto, no por
matemáticas: las matemáticas aquí solo están para divertirse. Ganar sería pura
chiripa.

## Cómo funciona

- Al arrancar sirve al instante con el CSV semilla empacado en el binario
  (`assets/Melate.csv`).
- Un task de fondo baja el CSV oficial al momento y **cada 48 horas**, valida,
  recalcula todo en memoria y respalda el archivo en Tigris. Sin base de
  datos: el CSV *es* la base de datos.
- Si Pronósticos no responde, se queda la última versión buena (o el respaldo
  del bucket).
- **Toda la matemática vive en Rust** (`src/stats.rs`): frecuencias, retrasos,
  co-ocurrencias, percentiles, las 8 estrategias y el calendario de sorteos.
  El navegador es un cliente delgado de la API y solo formatea.

## API

| Endpoint | Qué regresa |
|---|---|
| `GET /api/stats` | estadísticas por número, bolsa, próximo sorteo |
| `GET /api/tickets?strategies=hot,cold&count=8&k=6` | apuestas generadas (k 6–10 = múltiple) |
| `GET /health` | estado del servicio |

## Pruebas

```sh
cargo test
```

19 pruebas verifican que los cálculos son reales: fixtures calculados a mano
(frecuencias, retrasos, co-ocurrencias y percentiles de un histórico
sintético), invariantes sobre el CSV real, la combinatoria exacta
(C(56,6) = 32,468,436), la media del muestreo Gamma, las restricciones de
cada estrategia y la aritmética de calendario (algoritmo de días civiles).

## Correr local

```sh
cargo run --release
# http://localhost:8080  ·  estado en /salud
```

Sin variables de entorno corre sin respaldo (solo descarga directa). Para el
bucket: `AWS_ENDPOINT_URL_S3`, `AWS_REGION`, `AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY`, `BUCKET_NAME` (en Fly los inyecta `fly storage create`).

## Deploy

```sh
fly deploy
```

## Licencia

MIT. Los resultados históricos son datos públicos de Lotería Nacional.
