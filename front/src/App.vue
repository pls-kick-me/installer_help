<script setup>
import { onMounted, onUnmounted, ref, reactive } from 'vue'
import axios from 'axios'

const log = ref(null)
const log_content = ref('')
const loading = reactive({
    check: false
})

const event_source = new EventSource("/sse")

event_source.onmessage = (event) => {
    const evt = JSON.parse(event.data)
    log_content.value += evt.text + '<br>'
}

onMounted(() => {

})

onUnmounted(() => {
    event_source.close()
})

function check() {
    log_content.value = ''
    loading.check = true

    axios({ method: 'get', url: '/api/check' })
        .then(function (r) {
            console.log(r.data)
        })
        .catch(e => {
            console.log(e)
        })
        .finally(function () {
            loading.check = false
        })
}
</script>

<template>
    <div class="p-3 bg-secondary" style="min-height: 100vh;">
        <button @click.prevent="check" class="btn btn-warning me-3" :disabled="loading.check">Check</button>
        <div ref="log" v-html="log_content" class="log-container bg-dark text-white rounded-3 px-3 py-2 mt-3"></div>
    </div>
</template>

<style lang="scss">
.log-container {
    height: 400px;
    overflow-y: auto;
}
</style>
