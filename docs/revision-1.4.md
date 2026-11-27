# Tein 1.4 · Revisión de errores y seguridad

Revisión del código de descargas, entradas, portadas y componentes Android; pruebas de regresión y consulta de avisos RustSec del 27 de noviembre de 2026. No es una auditoría independiente ni una garantía de ausencia de vulnerabilidades.

| Hallazgo | Cambio y alcance |
|---|---|
| Una búsqueda anterior podía actualizar portadas después de empezar otra. | Cada búsqueda descarta los resultados y las miniaturas de generaciones anteriores. |
| Una página de error, una descarga incompleta o un cuerpo desmesurado podían quedar como canción. | Se comprueba el tamaño declarado cuando existe, se impone un máximo de 512 MiB por audio y se comprueba que el decodificador lo reconozca antes de publicarlo. No es una comprobación de integridad criptográfica. |
| Un nombre remoto podía representar un directorio en lugar de un nombre de lista. | Se sanea el nombre de archivo y de subcarpeta, se limita por bytes y se prueban componentes vacíos, puntos, separadores y Unicode. |
| Algunos enlaces se reconocían por contener el nombre del servicio en cualquier posición. | La entrada exige el prefijo HTTPS del proveedor (o una URI `spotify:`); el cliente de red solo permite HTTPS. El botón de SoundCloud no acepta un esquema ni un host arbitrarios. |
| Una portada pequeña en bytes podía necesitar mucha memoria al decodificarse. | La preparación de miniaturas y fotos elegidas aplica límites de dimensiones y un presupuesto de memoria al decodificador. Los límites de memoria dependen del formato. |
| Un dato de permiso ausente podía interpretarse como permiso de descarga en Audius. | Ahora hacen falta las tres señales explícitas: descargable, acceso a descarga concedido y sin restricción. En SoundCloud solo se guarda el MP3 publicado en el RSS del autor. |
| Los scripts tenían una contraseña de firma predeterminada. | La release usa un almacén existente y contraseñas de entorno. Los builds de desarrollo no generan una clave oficial nueva. No se encontró ningún almacén de firma en las revisiones de git disponibles. |

## Riesgos que quedan

- **Almacenamiento:** `MANAGE_EXTERNAL_STORAGE` es un permiso amplio. Se mantiene para editar o borrar música ajena a Tein y escribir por ruta en carpetas elegidas. No hace falta concederlo solo para reproducir. Una migración a MediaStore/SAF deberá probarse también con SD física.
- **Componentes Android:** la actividad de inicio es pública para el lanzador. El servicio de reproducción no está exportado.
- **Dependencias:** `cargo audit` revisó las 657 dependencias del lockfile, incluidas herramientas de compilación y otras plataformas. No encontró avisos de vulnerabilidades conocidos para las versiones fijadas; sí encontró ocho avisos de falta de mantenimiento: `bincode`, `chrono`, `paste`, `pin-utils`, `ttf-parser`, `zbus_names`, `zvariant` y `zvariant_derive`. Seguir sus reemplazos en los proyectos que las incluyen.
- **Firma:** quitar la contraseña de los scripts no la quita de revisiones anteriores. Proteger y respaldar el almacén y cambiar su contraseña fuera del repo; conservar la **misma clave de firma** para permitir actualizaciones.
- **Servicios externos:** que una búsqueda funcione no garantiza una descarga. Un autor puede retirar el archivo y una plataforma puede cambiar sus páginas, rechazar la red o limitar las pistas visibles.

## Disponibilidad y DDoS

Tein es un cliente: el APK no implementa un servidor de música ni abre un puerto de escucha en Internet. La biblioteca y el reproductor local funcionan sin un servidor. La protección del enlace a Internet y de los servicios remotos contra un DDoS volumétrico corresponde a sus operadores; el APK no puede garantizarla.

| Si ocurre… | Qué hace esta versión |
|---|---|
| Varias búsquedas seguidas | Ejecuta una a la vez, como máximo inicia una por segundo y conserva solo la última pendiente. Las miniaturas también tienen un solo trabajador. |
| Se encolan muchas canciones | Admite hasta 512 descargas activas o pendientes y solo transfiere una a la vez. Los temas restantes siguen disponibles para otro lote. |
| Se pide completar metadatos varias veces | Acepta un único lote en curso. |
| Un proveedor responde HTTP 429 o 503 | Pone en pausa las solicitudes al host inicialmente consultado. Usa `Retry-After` en segundos o fecha HTTP, con un intervalo de 5 segundos a 24 horas; sin un valor válido espera 30 segundos. Otros hosts no se bloquean. |
| Un servidor no responde o redirige repetidamente | Cada petición tiene un plazo total de 90 segundos y un máximo de cinco redirecciones. Esto no impone un plazo global a una playlist completa. |

Los límites de búsqueda y la pausa por host se comprueban con pruebas en memoria. La prueba de `429/503` simula las respuestas, verifica que el reintento no llega al transporte y que otro host sigue disponible; no genera tráfico de carga contra terceros.

## Verificación de la release

- El APK universal de 1.4.0 tiene el mismo certificado que 1.3.0. En un emulador Android 11 se actualizó desde 1.3.0 sin desinstalar, conservó la fecha de instalación y abrió con `armeabi-v7a`. El mismo APK abrió con `arm64-v8a` en un emulador Android 15. No son pruebas en un Samsung A13 ni con una tarjeta SD física.
- Pasaron las 14 pruebas sin red y las 14 pruebas de integración opcionales. La búsqueda de metadatos por nombre falló en una ejecución anterior con servicios externos y pasó al repetirla; esos proveedores no tienen disponibilidad garantizada.
