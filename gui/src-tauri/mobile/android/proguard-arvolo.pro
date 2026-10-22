# Quello che R8 non può sapere.
#
# In release il codice viene offuscato, e tre cose qui dentro sono chiamate
# **per nome da fuori Java**: Rust entra in `Shared.onShared` e `Shared.onTicket`
# attraverso JNI, cercando il simbolo `Java_it_termox_arvolo_Shared_onShared` —
# un nome composto dal package e dalla classe. Se R8 rinomina la classe, quel
# simbolo non esiste più e la condivisione da un'altra app, e l'apertura di un
# `.arvolo`, smettono di funzionare **in release e solo in release**: nessun
# errore di compilazione, nessun crash, semplicemente non succede niente.
#
# Le regole di default tengono i metodi nativi, ma non la classe che li ospita
# in tutte le configurazioni, e il costo di sbagliare è un difetto che si vede
# solo su un APK firmato — cioè dopo. Qui è scritto per esteso.
-keep class it.termox.arvolo.Shared { *; }
-keep class it.termox.arvolo.MainActivity { *; }
# E `Photos`, che è l'altro verso: non è Rust a essere chiamato da Java, è Java
# a essere chiamato da Rust — `find_class("it/termox/arvolo/Photos")` e poi
# `call_static_method(.., "recent", ..)`. Senza questa riga R8 tiene il nome
# della classe (qualcosa lo referenzia) ma rinomina i metodi in una lettera, e
# la galleria e il «Condividi» falliscono con un NoSuchMethodError che si vede
# solo su un APK di release. Verificato: senza, in `classes.dex` non ci sono né
# `recent` né `export` né `shareText`.
-keep class it.termox.arvolo.Photos { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
